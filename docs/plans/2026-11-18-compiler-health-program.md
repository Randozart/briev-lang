# Compiler health program — 2026-11-18

**Status:** PLAN / research complete. Nothing built, nothing modified except this
document and its sibling triage note (`docs/plans/2026-11-18-open-pr-triage.md`).

This is the *program*: what to do, in what order, and what mechanism each step
leaves behind so the same defect cannot come back. The per-PR dispositions live
in the triage note; this document sets priority and states the fix designs.

---

## 1. Where the compiler stands

| Surface | State |
|---|---|
| `main` build (`--all-targets`) | **RED** — `error[E0063]: missing field 'march_override'` in `src/compile.rs::webstack_opts` (bin test cfg). CI run `44419325395`. |
| `main` library tests | green (3935) |
| Native/parity/browser/determinism CI jobs | green |
| Five open PRs | one obsolete, four genuine (see triage note) |
| Issue #133 | **P0 correctness**: 5-line program SIGSEGVs at runtime; `check` says nothing |

Two independent things are wrong, and they compound: a name-resolution defect
that makes codegen emit a call the callee cannot accept, and a verification
apparatus — ours and the toolchain's — that cannot see it.

---

## 2. The principle for this program

> Fix the class, not the instance. Every item must end with a mechanism that is
> **capable of failing** — a check that has been demonstrated to catch the
> defect it exists for.

This session alone produced three instances of "a hand-rolled literal drifted":
`verify_ir`, `deny_accel_cpu_fallback`+`validate_ir`, `march_override`. Each was
fixed as an instance. The fix for the class is a gate that fails, not another
diligent patch.

---

## 3. P0-1 — Unbreak `main` (blocking; do alone, first)

Every branch cut from `main` is red for a reason unrelated to its own change, so
nothing else can be reviewed honestly until this is green.

`src/compile.rs::webstack_opts` states 58 of 59 fields. Measured deltas from
`BuildOptions::default()` are only `file_path`, `backend: Webstack`,
`int_bits: 32`. Replace the literal with those three plus a spread:

```rust
fn webstack_opts(file_path: &str) -> BuildOptions {
    BuildOptions {
        file_path: file_path.to_string(),
        backend: BackendKind::Webstack,
        int_bits: 32,
        ..BuildOptions::default()
    }
}
```

Acceptance:
- `cargo check -p briev-compiler --all-targets` clean;
- `cargo test --bins -p briev-compiler` compiles; `compile_view` tests pass;
- CI `Workspace tests` + `Default gates` green on `main`.

Why the spread and not "add the field": the doc comment on `impl Default for
BuildOptions` already states the rule ("literals that duplicate this field set
piecewise … only `check_source_for` is migrated here"). A 4-line literal cannot
rot again. This also makes PR **#129** a genuine no-op → close it, pointing at
this commit.

## 4. P0-2 — Close the gate gap that let it through three times

`scripts/gate.sh quick` → `scripts/gate.py` runs test *suites*
(`gate_selection.py::phases`: compiler lib + `QUICK_INTEGRATIONS` + verifier).
None of them compile the **bin target's `#[cfg(test)]` cfg**, which is the only
place `webstack_opts` exists. That is precisely the blind spot.

Add a compile phase, cheapest-first:
- `scripts/gate.sh quick`: `cargo check -p briev-compiler --all-targets` before
  `exec python3 -B scripts/gate.py quick`; or
- a `Check` pseudo-suite in `gate_selection.phases` so the manifest records it
  (consistent with how the gate already accounts for missing tools).

Acceptance — **the gate must be proven to bite**: temporarily delete a field from
`webstack_opts`, run `bash scripts/gate.sh quick`, confirm it fails, restore.
Record that demonstration in the commit message. An untested gate is decoration.

## 5. P0-3 — Issue #133: RESOLVED (2026-11-18)

**Landed in two commits** (`81d3c0f22` emitted-module check, `603360af7` frontend
fix). The section below is kept as the record of the investigation, with the
outcome appended in §5.5.

## 5. P0-3 — Issue #133: name collision → ABI mismatch → SIGSEGV

### 5.1 Mechanism (proven, not hypothesised)

The issue offered two candidates: link-time corruption, or stack exhaustion from
the reactor loop. Both are refuted, and the real mechanism is visible in the
emitted module.

`./target/debug/brievc check --validate /tmp/prcheck/sb1.bv` → `OK (emitted IR
verified)` (so it is *not* an IR-validity failure), and the `.ll` contains:

```llvm
define i64 @len(i64 %arg0) {            ; the StringBuilder overload
  %t3 = inttoptr i64 %arg0 to ptr
  %t4 = getelementptr i8, ptr %t3, i64 0
  %t5 = load ptr, ptr %t4               ; .buffer = first field
  %t0 = call i64 @briev_char_len(ptr %t5)
  ret i64 %t0
}
...
  %t10 = bitcast <{ i64, [4 x i8] }>* @str.28 to ptr   ; the "abc" literal
  %t8  = call i64 @len(ptr %t10)                       ; ptr where i64 is declared
```

At runtime `%arg0` is the address of the string-literal global; `inttoptr` to
`StringBuilder*`, `load ptr` reads the literal's stored length word (`i64 3`) as
a pointer, and `briev_char_len(0x3)` dereferences address 3 → SIGSEGV. The
symptom is fully explained, and it explains why importing `std/string.bv` fixes
it: then `len` is the String overload.

Narrowed by direct experiment (`brievc check`): the leniency is **specific to
`len`**. Correctly rejected: `append_str("abc", "def")`, `let b: StringBuilder =
"abc"`, a user `obj Wrapper { v: String }` parameter (also when the obj is
imported from another module), `hashmap`'s generic `len<K,V>` given a String.
`len("abc")` is accepted — with `StringBuilder`'s `len` as the only visible
definition. So the typechecker binds a callee that matches the *argument*, while
codegen binds the one definition the program contains. **Two resolvers, one
namespace.**

### 5.2 The toolchain cannot see this either — verified fact, doctrine change

LLVM 23.1.5 accepts call sites that do not match the callee:

```
define i64 @f(i64 %x) { ret i64 %x }
define i64 @g(ptr %p) { %r = call i64 @f(ptr %p) ... }   ; opt -passes=verify: exit 0
                                                          ; llvm-as: exit 0
                                                          ; clang -c: exit 0
                                                          ; call i64 @f() (0 args): also exit 0
```

Result-type mismatches *are* caught (`ret i32 0` in a `void` function errors), so
the verifier runs — it simply does not police call/definition agreement any more.
**Therefore `opt -passes=verify` must never be treated as an ABI check**, and the
`--validate` gate we shipped for issue #5 has this hole by construction. This is
doctrine, not a bug report: record it where the next author will look.

### 5.3 Fix design — three layers, ordered by value

**Layer A — ABI agreement check in `src/ir_verify.rs` (do first; turns the crash
into a diagnostic).**
`ir_verify.rs` already owns exactly this shape: `undefined_call_targets(ir)` is a
toolchain-free textual pre-check inside `verify_llvm_ir` (with the comment "a
call target with no `define` and no `declare` is invalid IR whatever LLVM
thinks"). Add a sibling `mismatched_call_signatures(ir)`:

1. pass 1 collects `define`/`declare` signatures → `(name → (ret, [param types]))`;
2. pass 2 collects `call <ret> @name(<args>)` sites and compares count + types;
3. on mismatch, render through the existing `render(...)` so the message names
   the call site, both signatures, and states the defect is the compiler's.

Because every path (`build`, `--llvm`, `check --validate`) funnels through
`verify_emitted_ir` → `verify_llvm_ir`, this single addition makes the #133 repro
fail *at compile time*. It also hard-stops the whole class (any future
frontend/codegen disagreement), which is why it goes first even though it is the
symptom-level fix: it converts silent miscompilation into a diagnostic while the
root cause is being found. Note it must not false-positive on the legitimate
`declare`-only protocol bindings — mirror `undefined_call_targets`'s treatment.

**Layer B — one resolver: bind calls to a definition (root cause).**
Locate the lenient path. The evidence points at a fallback that accepts a call
without a matching `defn`; the prelude even documents such a mechanism
(`plugins/parsed/prelude-native.bv:24`: "the typechecker's name-fallback lets
`List<T>` typecheck but the backend has no obj_members"). Discovery step: dump
the resolved callee for `len("abc")` under `string_builder` and find the site
that accepted it. Candidate places: the call-resolution fallback in
`src/typechecker/`, or a synthesized protocol member (`op Count`/`.^Length`)
being treated as a definition.
The rule to impose: **a call resolves to one definition, and that is the
definition codegen emits.** Concretely — the emitted symbol table is name-keyed
(`@len`), so any compilation unit containing two definitions of one name is
unsound *regardless of import form*; the typechecker must not be more permissive
than that table.

**Layer C — stdlib conforms to SPEC §881 (removes the ammunition).**
SPEC §881: "There is no overloading: the unqualified namespace holds one
declaration per name, across modules as well as within one." Today:

| name | defined in |
|---|---|
| `len` | `string.bv:44`, `string_builder.bv:57`, `hashmap.bv:30` |
| `to_string` | `string.bv`, `string_builder.bv:47` |

The resolver *does* diagnose this (`src/import_resolver.rs:586`,
`check_cross_module_collisions`) but only when both modules are explicitly
imported; the codebase works around it with selective imports
(`string.bv:16`, `json.bv:29` — `json_builder_string: to_string`). Options:

- **(preferred) shorten the surface**: `builder_len`, `builder_to_string`,
  `map_len` — mechanical, matches the "one name, one meaning" rule, and every
  existing selective import keeps working;
- or route length through `op Count()` per obj and keep one generic `len`
  (correct long-term, but touches the collection protocol — a bigger change than
  this program needs).

Add a **census gate**: a test that walks `lib/` defn declarations and fails on a
duplicate top-level name. Cheap, and it makes SPEC §881 enforced rather than
aspirational.

### 5.4 Regression fixtures (each must fail before, pass after)

1. `tests/tier1/` (or `tests/diagnostics/`) — the exact 5-line repro. After
   Layer C it must be a **compile error** naming the conflict ("`len` is defined
   by `std/string_builder.bv` as a `StringBuilder` operation; a `String` has
   `std/string.bv`'s"). It must never again be a program that builds.
2. `src/ir_verify.rs` unit test — a module whose call args do not match its
   definition is reported (`a_call_whose_arguments_do_not_match_its_definition_is_caught`),
   and a well-formed module still passes.
3. Duplicate-name census test over `lib/`.

Acceptance: #133's program cannot be built (diagnostic, not SIGSEGV); the class
it belongs to is caught by `ir_verify`; the census gate is green after the
renames; `bash scripts/gate.sh quick` and the full corpus census unchanged
otherwise.

## 6. P1 — Land the four dogfooding PRs

Per-PR detail and the exact deltas are in `2026-11-18-open-pr-triage.md`. Summary
of what changes before each lands:

| PR | Delta needed |
|---|---|
| #126 `check --stdlib-path` | error (not silent `break`) on a missing value; drop the redundant `BRIEV_STDLIB_PATH` fallback (the resolver already reads it as rung 2, `import_resolver.rs:425`) |
| #125 subcommand `--help` | restrict the scan to tokens before the first `--` (else `brievc run app.bv -- --help` is hijacked); explicit help on **stdout**, misuse on stderr; add the test `tests/cli_args_test.rs` already has the harness for |
| #127 `beginprogram` convergence | widen from `matches!(pre, BeginProgram)` to a recursive contains-`beginprogram` walk — `[beginprogram && done == 0]` warns today and the current arm does not fix it; add the conjunct test |
| #128 `net/server_loop.bv` | verify with `brievc check`; review three contracts (`serve_accept [true]` → `[listener.fd >= -4096]`; `serve_bounded`'s `term.len == 0` post; `buf_as_string`'s vacuous `x == 0 || x > 0`); hard-coded port 39481 needs a bind-failure skip |
| #129 `webstack_opts` | close as superseded by P0-1 |

Note #128 is *blocked in value* by #133: a server that cannot return a JSON
`String` can only answer numeric bodies. Land it, but do not advertise it until
the String path is fixed.

## 7. P2 — Hygiene that keeps the knowledge

1. **Close #5, #7, #8 with evidence.** All three have landed fixes
   (`e96a3ace7` `--validate` for #5; `76848f4de` for #7 Phase 1 + #8) but stay
   open. Leave a closing comment stating what was fixed, which fixture pins it,
   and what is explicitly *not* covered (issue #7 Phase 3 remains gated on Phase
   2 evidence). An open issue whose fix has shipped teaches readers to
   distrust the tracker.
2. **A dogfooding ledger.** Four real gaps (G0, G1, G4 and the server lifecycle)
   currently live only in PR descriptions; the fifth is already lost. Record them
   in one file with status, so the next dogfood run appends instead of
   rediscovering.
3. **#132 next** (glue `protocols.bv` never imports the module defining its ops)
   — same shape as #133: a generated artifact referring to a symbol nothing
   imports. The `ir_verify` work in P0-3 Layer A is a natural home for a check.

## 8. Order and acceptance

| # | Step | Why now | Acceptance |
|---|---|---|---|
| 1 | `webstack_opts` spread | unblocks all review | `--all-targets` clean; CI green |
| 2 | quick gate learns `--all-targets` | makes #1 permanent | gate fails when a field is removed (demonstrated) |
| 3 | `ir_verify::mismatched_call_signatures` | converts #133 to a diagnostic | new unit test fails before / passes after; corpus unaffected |
| 4 | find + close the lenient call path | root cause | #133 repro diagnoses instead of running |
| 5 | stdlib duplicate names + census gate | removes ammunition | census green; `check` on the repro errors |
| 6 | close #129 | hygiene | closed with pointer |
| 7 | land #126, #125, #127 | value, low risk | deltas above |
| 8 | verify + land #128 | stdlib surface | `brievc check` OK; loopback test passes twice |
| 9 | close #5/#7/#8; ledger; #132 | knowledge | tracker reflects reality |

Steps 1–2 are hours. Step 3 is the highest value per line in the plan. Steps 4–5
are the actual repair. 6–9 are the queue.

## 9. Effort and risk

- **Steps 1–2**: low risk, no semantic change. The only judgement is *where* the
  `--all-targets` phase lives.
- **Step 3**: low risk, additive (a new function beside `undefined_call_targets`,
  gated by the same "does not fire on legitimate modules" test discipline). The
  one design risk is IR text parsing: keep it structural (definition line vs call
  line), not a general parser, and require a fixture with `declare`-only protocol
  bindings to prove no false positive.
- **Step 4**: the unknown. The lenient path is not yet located; if the fallback
  turns out to be deliberately permissive for protocol members, the change must
  be narrowed to "no definition ⇒ error unless a protocol op supplies the body",
  and that needs its own review.
- **Step 5**: mechanical but touches stdlib importers (`string.bv`, `char.bv`,
  `json.bv`, and anything calling `len(builder)`). Behaviour-preserving; the
  corpus census is the check. Renaming is a **source-breaking** change for any
  out-of-tree program using `len(builder)` — acceptable because there is no
  released compiler, and the public name was already ambiguous.

## 10. What "the best possible place" means, measurably

Done when:

1. `main` is green, and the quick gate has been *observed* to fail on the defect
   it exists for.
2. No program can pass `check`/`build` with a call that does not match its
   callee (a compile diagnostic, with the two signatures named).
3. `lib/` has one definition per name, enforced by a test, not by convention.
4. The five open PRs are merged-with-hardening or closed-with-reason; #5/#7/#8 are
   closed with evidence; #133 is a diagnostic and a fixture.
5. Every dogfooding gap has a durable home.

## 11. Deliberately not doing

- **Not** making the convergence `"error"` severity fail the build
  (`src/pipeline.rs:1459` prints `warning: error:` on something advisory). It is
  advisory by an explicit "warn, migrate, then error" decision; flipping it is a
  separate judgement and would break the corpus.
- **Not** touching `is_reactive` at the parser for `beginprogram` nodes — it
  would ripple into `pgo`, `density`, `causality`, `global_lifetime`.
- **Not** relying on cross-toolchain LLVM behaviour: the verifier blind spot is
  recorded, and the project must own the invariant. Worth one run of
  `benchmarks/toolchain_matrix.sh` afterwards to see whether older clangs are
  stricter — as information, not as a substitute for Layer A.
- **Not** attempting #4 (macOS) or #2 (C-callable static library) here; both are
  separate campaigns.

### 5.5 Outcome — what was actually wrong

The exploration above assumed a **name-collision in the frontend**. That was
half right: the collision exists (`len` in three stdlib modules), but it is not
where the program escaped. The actual hole, found by instrumenting call
resolution and watching `infer_call` never run for `len`:

> **`infer_intrinsic_call` looped over `sig.parameters` instead of over the
> arguments.** A type-inferred intrinsic declares no parameters (`Print#`,
> `Alloc#`, `Store#`), so the loop body never ran and **no argument was
> inferred at all**. `Print#(len("abc"))` reached codegen with `len` never
> typechecked, and codegen bound the one definition in the program — the
> `StringBuilder` one.

Proof before the fix: `len("abc")` in `term`/`let` position was *correctly*
rejected (`expected StringBuilder … found String`); the same call inside
`Print#(...)` was accepted. Same expression, two verdicts, because one position
was typechecked and the other was not.

That also dissolves the `ptr`-vs-`i64` question in §5.3 Layer A: the checker and
the emitter disagreed because the checker never looked. Once every argument is
inferred, the argument is validated against the definition the emitter will bind
(one definition per name is already enforced by
`import_resolver::check_cross_module_collisions`), so the two cannot disagree.

**Fixes landed:**

| layer | change |
|---|---|
| frontend | infer every argument (`infer_intrinsic_call` walks `args`), skipping only a bare identifier in an untyped position (a token: `Alloc#(n, Malloc)`) |
| frontend | expectation-directed argument inference (`infer_argument`): a variant construction learns its unbound params from the declared parameter, so `classify(Ok(A(7)))` against `Result<J, String>` stops comparing as `Result<J, E>` |
| prover input | intrinsic range facts: `Load#(p, W)` is in `[0, 2^(8W)-1]` — the interval prover has no arithmetic for `x & 255`, so byte-load-fed `[b >= 0]` preconditions were unprovable |
| emitted IR | `mismatched_call_signatures`: arity + representation class, toolchain-free, wired into `verify_emitted_ir` |
| stdlib | `std/net/websocket.bv` states the `[off >= 0]` it always assumed |
| fixtures | `hello/main.bv` (`.#Size`→`.^Length`), `error-handling.bv` (Result→Int), `net-bytes-sink.bv` + `byte_sink_trait_smoke.bv` (length guards), `deterministic_transitions.rs` (retired `GetEnvInt#`), `structured_tests.rs` (Ptr+Int is not an operation) |

**Measured:** `brievc build` on the issue's program now exits with
`type mismatch: expected StringBuilder for argument 0 of 'len', found String`
and writes no binary. Corpus `check`: 274 ok / 46 fail — identical to the
pre-change baseline, so the fix rejects nothing legitimate. lib/std census
unchanged (`files_failing` 0, `goals_violated` 0). lib suite 3939/3939,
`gate.sh quick` green, determinism byte-identical.

**Two of the pre-existing ill-typed calls found on the way are still open** and
belong to the call-site coercion retirements below, not to #133:
`char_to_str` is called with an `i32` (truncated) against an `i64` declaration
(7 programs), and five handle/function-value positions pass `ptr` where `i64` is
declared (or the reverse). Both alias one register, so nothing crashes today;
the emitted-module check exempts the widths deliberately and pins the exemption
with `integer_class_aliases_are_deliberate_and_stay_exempt`.

### 5.6 Revised conclusion on §5.3 Layer C (stdlib duplicate names)

Reclassified from *correctness* to *hygiene*. The resolver already refuses a
program in which two modules contribute the same name
(`import name 'to_string' conflicts … one shared root namespace means the winner
would depend on import order`), so a compiled program has one definition per
name — which is now also the one the checker validates against. Renaming
`len`/`to_string` would therefore buy no safety, and a census gate would force a
mechanical rename of the collection/string/string-builder surface with no
behavioural gain. Recorded here as a cleanup, not a P0.
