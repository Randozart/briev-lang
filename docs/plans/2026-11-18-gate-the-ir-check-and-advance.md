# Next queue: gate the IR check, fix the named bugs, advance the pin (2026-11-18)

Follows `2026-11-18-compiler-diagnostics-ir-verification.md` (landed at `c06804413`).
That session built the instrument; this one makes it enforced and spends its
findings.

## P0. Sync first — the worktree is behind

```
local HEAD   c06804413   (intel-xpu-harvest)
origin/main  e5849e192   (another session has pushed)
```

Step 0 of every item below is `bash scripts/sync-main.sh` from
`/home/randozart/Desktop/Projects/briev-intel-xpu` (it refuses a dirty tree), then
`bash scripts/gate.sh quick` to confirm the merged state is green before touching
anything. If a main-side commit touches `src/compile.rs`, `src/ir_verify.rs` or
`src/backend/spirv/`, re-read it before editing — those are the files this plan
touches.

## P1. Make the IR check a gate, not a tool (highest leverage)

`scripts/ir_verify_corpus.sh` exists and works; nothing runs it.

1. **Parallelise it.** Two `brievc build` passes per file over `lib/` exceeded a
   40-minute window. Restructure around `xargs -P "$(nproc)"` with per-file output
   dirs (the script already keys by path) and a summary assembled from the per-file
   verdicts. Target: whole tree (lib + tests + examples) under ~10 minutes warm.
2. **Add a workflow** mirroring `.github/workflows/verify-bodies-census.yml` (same
   ubuntu-24.04 + clang/llvm/z3 install + rust-cache + `cargo build --release`):
   run the sweep, upload the log, and **fail on `files_invalid_ir > 0`**.
   Non-blocking at first? No — the current count is 3, all known, so either fix them
   (P2 + P4) and gate at zero, or gate at a checked-in allowance list that shrinks.
   A number that can grow silently is how we got here.
3. Consider wiring the fast subset (`tests/`, ~2 min) into `gate.sh quick` so a
   codegen change fails locally, before the push.

**Allowance file** (if the fix order slips): `tests/ir_verify_allow.txt`, one path
per line, sorted, with a comment per entry naming the tracker. The workflow fails if
the set of failing files ≠ the allowance — so it catches BOTH a new regression and a
stale exemption. That is the ratchet shape used for `--deny-memory-ambiguity`.

## P2. `@str_to_c` is called but never defined (two fixtures)

Measured on `tests/fixtures/glue_gen/parity_handwritten.bv`:

```llvm
declare i64 @c_strlen(i64) #6          ; the frgn is declared
...
  %t15 = call ptr @str_to_c(ptr %t12)  ; no declare, no define
  %t17 = call i64 @str_to_c(ptr %t19)
```

`str_to_c` is a Briev `defn` reached through a protocol axiom
(`axiom CastFrom(String<UTF8>) = str_to_c(#Lh);` in
`tests/fixtures/glue_gen/handwritten_glue.bv`). The call site is emitted, the callee
is not — i.e. **defn liveness does not treat a protocol-axiom body as a live root**.
Same file pair also fails as a pair (`parity_generated.bv` is the generated twin).

Two changes, both worth having:

1. **Root the axiom callee.** Liveness must consider a protocol axiom's expansion a
   root when any cast using it is emitted. This is the real defect: a call we emit
   must keep its callee.
2. **Never emit a call to an undefined symbol.** A defensive pass at the end of
   `generate()`: any `call`/`@name` reference with no `define`/`declare` in the
   module becomes a `declare` of an inferred signature — which turns silent invalid
   IR into an honest link error naming the symbol. This also protects the 17 BUGS
   A–E entries while they are being fixed.

Gate: both fixtures compile, link and run under `tests/glue_gen_parity.rs`; the
sweep drops to 1 (only `defn-composite`).

## P3. Advance the kiosk pin

`briev-kiosk-avatar/compiler.lock` pins `commit = 3e72d9313`; the lane has landed
the accel-surface work, the f16 fix, the residency diagnosis and the diagnostics
work since. Bump `commit` to the post-P4 HEAD, update the trailing comment (what the
pin buys), and leave `base_main` alone unless main moved materially. Then re-run the
kiosk soak/census (`harness/soak.py`, `harness/memcheck_ratchet.sh`) so the pin is
validated, not just bumped.

## P4. D1 — the loop-carried `Float`, with the new feedback loop

Two approaches are ruled out and recorded in `BUGS.md`: the compositor layer (loses
the deferred-softmax plan) and the countable-loop preheader (the binding does not
reach the inner loop). The remaining target is the **inner `foreach`'s assignment
path**, where the composite's local is demoted to its own `alloca i64`.

Next experiment (cheap, decisive): in `collect_reassigned_lets` and in the countable
preheader block, log the names collected for `defn-composite.bv`. If `acc_` is absent,
the body arrives as `Expr::Block` and the walk misses it; if present, the inner
foreach's emitter is clearing `let_bindings` before the assign and re-allocating.
Either answer names the one function to change.

The new `ir_verify` output makes this a seconds-long loop: build →
`%t36 is defined at …` → read the two hops → change one thing.

Fallbacks if the demotion site resists: (a) teach `emit_assign_statement` to consult
`let_original_types` before allocating a fresh slot (declared type wins), (b) post-pass
retyping of the slot when the only stores are float values — last resort, and it must
come with the arithmetic gate, because retyping changes values.

Verification for P4 (unchanged from the plan of record): `defn-composite.bv` produces
clang-valid IR **and** runs correctly; a tier1 fixture with a composite float
accumulator prints its value; `bash benchmarks/build_and_bench.sh --runtime` A/B
before/after with both sides recorded (Rule 20 — a regression refutes the hypothesis).

## Order, cost, gates

| # | item | time | gate |
|---|---|---|---|
| 0 | sync to `e5849e192`, gate quick | 15 min | gate green on merged state |
| 1 | parallelise the sweep; add the workflow + allowance ratchet | 2 h | whole-tree sweep < 10 min; CI red on a new invalid module |
| 2 | root the axiom callee; defensive `declare` for undefined calls | 2–3 h | the two glue_gen fixtures build, link, run |
| 3 | bump `compiler.lock` + kiosk soak/census | 1 h | pin validated by the kiosk harness |
| 4 | D1 inner-foreach demotion | 3–5 h | valid IR, right value, A/B clean |

## Doctrine notes

* **P1 first.** Every item after it is cheaper once a regression fails in CI rather
  than in a session like the last one, where three invalid-IR files sat unnoticed in
  two different directories.
* **P2's second fix is the interesting one.** Rooting the axiom callee fixes today's
  two files; "never emit a call to an undefined symbol" fixes the class, including
  the 17 BUGS A–E entries nobody has been able to action individually.
* **P4 must not retype values as a shortcut.** The slot is part of the arithmetic;
  the fix is to type it from the declaration, and the A/B is what decides.
