# FFI follow-ups — what the contract grind deliberately left out

**2026-11-16.** Status: **follow-up plan — not started.** Companion to
`2026-11-16-ffi-remaining-grind.md`, which says in its own words that the items
below are real work but *not* the FFI contract, and that mixing them in would make
"done" unfalsifiable.

This document exists so they are queued rather than remembered, and so the
distinction is explicit: **that grind proves the FFI design; this backlog pays
down the debt the design sits on.**

---

## 0. Why these were excluded

The grind's definition of done is falsifiable on its own terms (generated ≡
hand-written IR, HIR reports the contract, Python needs no Rust, no live removed
surface). None of the items here move that needle, and several are open-ended
(an external reproducer that may never arrive; a toolchain CI that needs
infrastructure). Mixing them would mean the grind could never close.

They are ordered below by **risk to the FFI work's own claims**, not by age.

---

## 1. Triage

| # | Item | Why it matters to the FFI contract | Size | Blocker |
|---|---|---|---|---|
| F1 | **IR not reproducible across processes** (BUGS: state-field index + interned-string order vary) | The parity gate compares emitted IR byte-for-byte. If emission is process-dependent, the gate is either flaky or masked by luck. This is the one item that can invalidate a grind result. | M | none |
| F2 | **Data→Float width mismatch emits an invalid bitcast** (BUGS, OPEN) | Boundary lanes are exactly where width mismatches live; a validator that emits invalid IR poisons any descriptor-driven path. | S | none |
| F3 | **Corpus floor: 17 host IR regressions (groups A–E) + 2 string bugs** | The verification corpus is the ground truth the FFI work is measured against. 34 ERR files mean `--verify-bodies` is partly blind. | L | none |
| F4 | **Missing stdlib import silently binds a name to a builtin of a different type** (BUGS) | Descriptor-driven glue *generates* declarations; if a missing import can silently resolve to a builtin, generated code can too. Silent wrong binding is the failure mode the whole plan forbids. | M | none |
| F5 | **Toolchain matrix on CI (≥2 LLVM/Clang)** | S2/S3 prove data-driven glue emits *identical* IR; cross-toolchain behaviour is a different property, and today it is measured only when someone remembers to run the script locally. | M | CI host with 2+ clangs |
| F6 | **HIR port queue** (`reclamation`, `electronics`, `destination_ownership`, `accel`, `gpu_schedule` still AST-first) | The FFI design *extends HIR* (H1/H2). Every family still reading the AST is a consumer that will not see the new boundary facts. | L | grind's H1/H2 landing |
| F7 | **Casting-graph Kani proofs** (`find_path` / `resolve_llvm_type`) | The casting graph is what turns a descriptor into a marshalling lane. A wrong path is a wrong boundary with no diagnostic. | M | none |
| F8 | **Issue #2 v2 ABI** (Bool/String/struct marshalling + generated headers) | The export direction of the same contract; v1 static-library export is verified, v2 is not. | L | G2/G3 (generated headers) |
| F9 | **Issues #5 / #7 / #8 — no reproducers** | Reported defects with no fixture. Cannot be planned as work, only *requested*. | — | external reporter |
| F10 | **Self-tail call leaks every iteration's owned temporary (quadratic RSS)** | Ownership of returned buffers is exactly what the boundary contract declares; the leak shows the ownership model is not yet enforced everywhere. | M | none |
| F11 | **`h5-phase-c4` branch** (28 hunks / 1359 lines in `emit_expr.rs`) | Finished work that cannot be merged; needs its owner to rebase. Not ours to force. | — | branch owner |

---

## 2. Order

**Wave 0 — protect the grind's results (do first, small):**
`F1`, then `F2`, then `F7`. Each is small, self-contained, and each one either
strengthens or directly enables the parity gate. `F1` first because a parity gate
on non-reproducible output is a coin flip.

**Wave 1 — make the verification ground truth trustworthy:**
`F4` (silent wrong binding), then `F3` in slices (group A–E one at a time, each
with a fixture; the corpus floor moves visibly per slice).

**Wave 2 — close the design's own edges:**
`F6` (HIR port queue) once H1/H2 have landed, starting with
`destination_ownership` (the family the boundary facts feed); then `F10`
(ownership enforcement, which the leak proves is incomplete).

**Wave 3 — reach and infrastructure:**
`F8` (export v2 ABI, now with generated headers from `G2`), `F5` (toolchain CI —
needs the host, so schedule when infrastructure exists), `F9` (only when a
reproducer arrives), `F11` (owner's call).

---

## 3. Per-item detail

### F1 — IR reproducibility (do not defer)

BUGS records state-field index and interned-string order varying per process. The
determinism gate passes on three programs, so the defect is either intermittent or
absent from those three — both possibilities are bad for a byte-comparison gate.

- **Work:** reproduce with the recorded case; find the `HashMap` iteration feeding
  field indices or the string interner; sort it (project rule: every collection
  feeding IR is ordered). Extend `benchmarks/ll_determinism_check.sh --quick`
  corpus with the recorded program.
- **Gate:** the recorded program emits byte-identical `.ll` across ≥2 processes ×
  N runs; the quick corpus grows by that program.

### F2 — `Data`→`Float` invalid bitcast

- **Work:** the validator emits a bitcast whose operand and destination widths
  disagree; make the cast lane go through the `CastingGraph::find_path` result
  (per backend-type-dispatch doctrine, no ad-hoc bitcast at the site).
- **Gate:** the recorded program emits valid IR (llc accepts it) and the value
  round-trips; a regression fixture pins it.

### F3 — corpus floor (slices)

The 17 host IR regressions are grouped A–E in BUGS; each group is one slice.

- **Per slice:** reproduce the group's fixture, fix, and move the file from ERR to
  PROVEN/UNPROVEN in `scripts/verify_bodies_corpus.sh`'s HOST_EXCLUDES accounting.
  A slice is done only when the corpus floor line in `bug`-commentary is updated —
  otherwise "fixed" is unfalsifiable.
- **Gate:** corpus floor printed before/after per slice; no slice may add a new
  ERR.

### F4 — silent builtin shadowing on a missing import

- **Work:** a name that resolves to a builtin because an import is missing is the
  same class as a keyword silently changing meaning. Make the resolution
  diagnostic: "`x` is not imported; the compiler would bind it to builtin `x`
  (type T) — did you mean to import? `. Add the import or rename."
- **Gate:** a compile-fail fixture per case; the corpus builds unchanged.

### F5 — toolchain matrix in CI

`benchmarks/toolchain_matrix.sh` already does per-clang verdicts; it needs a host
with ≥2 versions. When that exists: run on backend-touching commits, publish the
verdict table as an artifact, and fail only on a *verdict flip* versus the
recorded baseline (not on a version being absent).

- **Gate:** the workflow fails when a corpus row changes verdict against the
  committed baseline table; `ALLOW_TOOLCHAIN_MISMATCH=1` documents an intentional
  flip.

### F6 — HIR port queue

Order by "consumes boundary facts": `destination_ownership` → `reclamation` →
`electronics` → `accel` → `gpu_schedule`.

- **Per family:** build the HIR twin, keep the AST path as the debug oracle with
  a drift assertion (the established pattern: `debug_assert_eq!` on rendered
  output), then flip the consumer.
- **Gate:** family-specific parity test green with the oracle assertion active;
  `--lib` green.

### F7 — casting-graph Kani proofs

- **Work:** `kani/prove_cast_path.rs` — properties worth proving: `find_path`
  never returns a path shorter than 1 for identical types (identity exists);
  every step's source/target types are adjacent in the graph; `resolve_llvm_type`
  is deterministic for the same `(universe, type, int_bits)`; no path is returned
  when the target protocol is unreachable.
- **Kani quirks (known):** Kani 0.67 needs plain `assert!`/`assert_eq!`, not
  `kani::assert!`; `kani/` is a standalone crate (`cd kani && kani prove_….rs`).
- **Gate:** harnesses pass; a deliberately broken adjacency check makes one fail
  (proving the harness bites).

### F8 — Issue #2 v2 ABI

- **Work:** Bool/String/struct marshalling across the export boundary with
  generated headers (`G2` produces them), and the v2 ABI shape agreed with the
  consumer. This is the export half of the same contract; the descriptor already
  carries `protocols:`/`conversions:` for it.
- **Gate:** a C consumer compiles against the generated header and calls a
  `export defn` taking Bool/String/struct, with results matching a native Briev
  run.

### F10 — tail-call owned-temporary leak

- **Work:** every iteration of a self-tail call retains the previous iteration's
  owned temporary. The reclamation plan should place the drop at the tail-call
  boundary; if it cannot, the ownership contract is incomplete and that is the
  finding.
- **Gate:** RSS is flat over a long self-tail loop (the recorded repro); allocation
  count in the emitted IR is constant per iteration, not cumulative.

### F9 / F11 — not actionable here

`F9` needs the external projects (`test-project-otto`, `transition_validate`) to
supply fixtures. `F11` needs the branch owner to rebase; the plan records the
measurement (28 hunks, 1359 lines) so nobody re-derives it.

---

## 4. What "done" means, per wave

| Wave | Done when |
|---|---|
| 0 | parity gate runs on reproducible IR, the invalid bitcast is gone, and the cast graph has proofs that bite |
| 1 | a missing import is a diagnostic, not a silent builtin binding; the corpus ERR floor has dropped by at least one group with the floor line updated |
| 2 | `destination_ownership` reads HIR; a long self-tail loop shows flat RSS |
| 3 | a C consumer compiles against generated headers and calls a Bool/String/struct export; the toolchain matrix fails on verdict flips in CI |

---

## 5. Risks

1. **F1 can invalidate a completed grind.** If emission is genuinely
   process-dependent, the G4 parity gate must be re-run after F1 — schedule them
   adjacently, not quarters apart.
2. **F3 is a fog, not a task.** 34 ERR files with group letters; each slice needs
   its own reproduce, and "the corpus is greener" is not a gate — the floor line
   is.
3. **F6 after H1/H2, never before.** Porting a family to HIR that lacks the
   boundary facts just relocates the blindness.
4. **F5 depends on the CI host.** Planning it as work would be pretending;
   planning the *baseline table* and the flip rule is real work that can be done
   now.
5. **F8's ABI shape is an agreement, not a discovery.** It needs the consumer's
   sign-off before implementation, like `D1` did for ownership.
