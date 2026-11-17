# Three open threads — investigation and plan (2026-11-17)

Follow-up to `docs/plans/2026-11-17-device-free-work-queue.md` and
`benchmarks/results/2026-11-17-corpus-err-triage.md`. Read-only investigation of
the three threads left open at the end of that pass:

1. class D — the two stdlib files that never compiled;
2. class C survivors — `ptr-arithmetic`, `ptr-swap`, `sig_as_type`;
3. the corpus floor that could not be reproduced.

### Two retractions of the previous pass's own record

`benchmarks/results/2026-11-17-corpus-err-triage.md` states two things this
investigation contradicts. Both were inferences from data that did not survive a
second look; the file is a timestamped record, so the correction lives here
rather than in it.

1. **"it is not an export-unwrapping bug — the export marker is a red herring".**
   Wrong. The A/B's control run produced no `.ll` at all, so it compared a
   failure with an unexplained failure. Thread 1 below shows `export` is precisely
   the shape: `export let` is skipped by both storage walks, and no other file in
   `lib/std/` uses that form.
2. **"21 ERR"** read as files. The corpus script's `ERR` counts *error lines*.
   The distinct failing files are about 18; one file could contribute several.

Both are the same mistake in different clothes: trusting a number whose unit was
never established. That is also thread 3's finding.

---

## Thread 1 — class D: `export let` is skipped by the two walks that matter

### Correction to the committed record first

The triage file says "it is **not** an export-unwrapping bug — the export marker
is a red herring", on the strength of an A/B where removing `export` also failed.
That conclusion does not hold: the control run's `.ll` was never written (the
build died earlier), so the A/B compared "failed" with "failed for an unknown
reason". The claim must be retracted, and it turns out to be exactly wrong.

### What the code says

`lib/std/posix/io.bv` has **15** `export let` declarations;
`lib/std/posix/process.bv` has 2. No other file under `lib/std/` uses the form —
which is why exactly these two stdlib files fail.

`TopLevel::Export` is unwrapped in seven places in `src/backend/llvm/mod.rs`
(lines 78, 121, 918, 4604, 4893, 5634, 10241) and in **zero** places in
`src/analysis/storage_class.rs`. The two walks that decide a module-level `let`'s
storage both iterate the statement form only:

* `classify_plain_lets` (`src/analysis/storage_class.rs:291`) —
  `if let TopLevel::Statement(stmt) = item`;
* the `let` arm of `build_field_index` (`src/backend/llvm/mod.rs:9334`) —
  `} else if let TopLevel::Statement(stmt) = item {`.

So an `export let` is classified by neither: it is not recorded in `plain_lets`
and not registered in `plain_global_types` (nor as a `%State` field).

### And the read path hides it

The identifier-read chain in `src/backend/llvm/emit_expr.rs` ends with an
unconditional last resort:

```rust
} else {
    writeln!(out, "{}{} = load i64, ptr @{}", indent, v, name).ok();
}
```

Any name that is not a phi, a plain let, a state field, or an `init` global is
*assumed* to be a module global and loaded. Nothing declared it, so the module
fails verification:

```
%t9 = load i64, ptr @FD_STDOUT     ; emitted
(grep -c "@FD_STDOUT = " == 0)     ; never declared
```

Three defects in series: two walks that skip `export`, and a fallback that
converts "I do not know this name" into emitted IR.

### Plan

**Step 1 (guard, do first).** Make the last-resort branch loud. An identifier
that reaches it is either a compiler bug or a program the frontend should have
rejected; emitting an undeclared global turns both into an LLVM verifier dump at
`opt` time, far from the cause. Follow the house rule for diagnostics: what is
wrong, why, and the fix. This alone converts the two failures from "invalid IR"
into an actionable message, and it is what stops a fourth instance of this class
from appearing silently.

**Step 2 (the actual fix).** Unwrap `Export` in the two storage walks, so
`export let` is classified and registered exactly like a bare `let`. Two
questions to settle before writing it:

* **Linkage.** An exported `let` is plausibly meant to be a linkable symbol. A
  module-scope `@name = global i64 1` is already external by default, so the
  plain-storage lowering probably satisfies it — but the export ABI
  (`compute_export_needs_state`, `export_abi`) may expect `%state`-backed access
  for exported *state*. Check what `lib/std/posix/io.bv` is used for through
  GLUE before choosing; if a consumer reads the symbol directly, the global must
  keep its declared name and type (it does).
* **Both walks or one.** Whichever way linkage goes, `classify_plain_lets` and
  `build_field_index` must agree — a name in `plain_lets` but not registered is
  the exact state that produced this bug. Add a debug assertion over the pair
  (every `plain_lets` member must have a `plain_global_types` entry after
  `build_field_index`), so a future divergence fails in CI.

**Step 3 (verify).** `lib/std/posix/io.bv` and `process.bv` build and link;
corpus ERR drops by 2 (plus whatever the `@SYS_CLONE` redefinition shares);
the 200+ other `lib/std` files are unchanged (compare PROVEN/ERR per file, not
just totals).

**Risk.** Only two files use `export let`, so the blast radius of Step 2 is tiny.
Step 1 touches every identifier read — but only by *replacing* an emission that
already produces invalid IR. Nothing that compiles today can reach it.

---

## Thread 2 — class C survivors: three sites, one family

All three are the same root as slice C ("a `Ptr`-stamped value may already BE a
pointer"), at three sites that were not fixed. Exact IR, from the current tree:

### C1 `ptr-arithmetic` — a state-field *initializer* stores a ptr into an i64 slot

```llvm
%ip_0 = getelementptr inbounds %State, ptr %state, i32 0, i32 0
%t0  = load ptr, ptr @UART_RX         ; the const is ptr-typed
store i64 %t0, ptr %ip_0, align 8     ; ← ptr stored as i64
```

The very next field proves the shape of the fix: its initializer is an
*expression* (`UART_RX + 4`), the arithmetic forces a `ptrtoint`, and the store
is well-typed. A bare-identifier initializer skips that adaptation. Site: the
initializer-value path shared by `emit_init_state` and
`emit_inline_init_stores`.

### C2 `sig_as_type` — the indirect call does not use the loaded signature

```llvm
%t1 = inttoptr i64 %ac1 to ptr
%t2 = load i64, ptr %t1              ; a fn value from a state slot
%t3 = inttoptr i64 %t2 to ptr
%t5 = inttoptr i64 %ac0 to ptr
%t6 = call i64 %t3(ptr %t1, i64 %t5) ; ← '%t5' defined with type 'ptr' but expected 'i64'
```

A higher-order value's parameter adaptation is guessed instead of read from the
function type the value carries. The `Function(params, ret)` type is on the
`TypedRegister`, so the call argument adapters can consult it the way they
consult `defn_param_tys` for direct calls.

### C3 `ptr-swap` — a state slot used as an lvalue through ptrtoint

```llvm
%t36 = getelementptr inbounds %State, ptr %state, i32 0, i32 0
%t37 = ptrtoint ptr %t36 to i64      ; the slot ADDRESS as i64
%t41 = load i64, ptr %t37, align 8   ; ← %t37 is i64, `load` needs ptr
```

Two state slots are swapped; the address is ptrtoint'd and then used where a
pointer is required. The ptrtoint is pure loss — the GEP result is already the
pointer the load wants.

### Plan

Fix them one at a time, in this order (cheapest first, each independently
verifiable):

1. **C3** — drop the redundant ptrtoint/`canonical_ptr` on a value that is
   already a pointer. Same `reg_value_ll` test as slice C; the site is the
   state-target adapter.
2. **C1** — adapt the initializer value to the field's LLVM type before the
   store (the `ptrtoint` the expression path already gets). Verify against
   `emit_init_state` and the inline variant, and check `%State` field stores
   elsewhere for the same asymmetry.
3. **C2** — read the callee's `Type::Function` parameter list for an indirect
   call's argument adaptation. Highest risk: it changes how every higher-order
   call marshals, so it needs the full suite plus the wasm dashboard artefacts
   in mind (the adapter comments cite wasm32 IR verification as the discoverer
   of the last two width bugs here).

Each fix: one example file goes clean, corpus ERR drops by one, floor re-measured,
`cargo nextest --lib` plus `daily_use_smoke` (the tier1 corpus is the real
regression net for backend changes — slice C regressed three fixtures on its first
attempt).

---

## Thread 3 — the floor: no bucket in the record shares a unit with any other

### What each tool actually counts

`verify_bodies_corpus.sh` tallies with one `awk` over interleaved output. Read
line by line (verified against the script, not remembered):

| bucket | unit | evidence |
|---|---|---|
| `PROVEN` | **file** — counts `FILE:` headers with no failure line | `/^FILE: / { if (seen && !bad) proven++ }` |
| `UNPROVEN` | **goal** — one per "could not prove:" line | `/could not prove:/ { unproven++ }` |
| `SKIPPED` | **goal** | `/verify-bodies skipped:/ { skipped++ }` |
| `VIOLATED` | **goal occurrence** | `/verify-bodies violation:/ { violated++ }` |
| `ERR` | **error line**, not file — a file with 3 errors counts 3 | `/error:/ { errors++ }` |

`SKIPPED: 34 560` beside `Files: 243` is the giveaway that at least one bucket is
not a file count.

`verify-bodies-census.sh` counts **goals** throughout, from the JSON report:
`len(d['proven'])`, `len(d['unproven'])`, …

### Why the record cannot be reconciled

The recorded sentence is "242 files / 210 PROVEN / 18 UNPROVEN / 34 ERR". Its four
numbers are: a file count, an unknown-unit count, an unknown-unit count, and
(a file count if it came from the corpus script, or a goal count from the census).
My own numbers from the last pass inherited the same disease: "60" is files,
"207" is goals, "21" is error *lines* (the distinct failing files are ~18).

So the honest finding is not "the floor moved" and not "the record used the wrong
script" — it is that **the corpus summary has never reported two numbers in the
same unit**, which makes every before/after claim in the campaign untestable.
That is the same class of defect as the shared `--out` directory and the dead-pid
soak: an instrument that produces numbers with no defined meaning.

### Plan

1. **Rewrite the tally to report both units, explicitly labelled**, and take the
   goal-level numbers from `--verify-report` JSON (the census's source) instead of
   grepping human-readable lines. One table, two columns:
   `files_{proven,failing,error}` and `goals_{proven,unproven,skipped,violated}`.
   The line-grep can stay for the fast path, but it must be labelled as
   goals-vs-lines when it is shown.
2. **Restate the floor once, in one results file, in both units**, and mark the
   old sentence superseded. Do not "correct" the old file in place — it is a
   timestamped record.
3. **Gate on the two unambiguous quantities only**: per-file `error` (a file that
   does not build) and per-goal `violated` (a contract falsely proved). `PROVEN`
   moves when prover coverage changes, so it is a progress indicator, never a
   gate.
4. Then, and only then, ask whether the historical 210/18 differed. The
   cross-check is one census run over the same file list, script-only.

**Why this outranks the remaining code fixes in importance, though not in size:**
every "the corpus is greener" claim in the last two sessions rests on numbers
that cannot be compared. Fixing the units is what makes the next fix provable.
