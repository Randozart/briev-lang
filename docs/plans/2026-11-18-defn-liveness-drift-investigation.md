# HIR/AST defn-liveness drift — investigation (2026-11-18)

**Verdict: the drift is real, and the recorded diagnosis in `BUGS.md` has the
direction backwards.** The cause is not a missing walk arm — it is a
**name-collision rule mismatch** between the two liveness builders.

## 1. Reproduction (resolved program)

`lib/std/os/dir.bv` compiled as root, with the prelude and imports resolved
(232 items):

| set | size |
|---|---|
| HIR live | **79** |
| AST live | **24** |
| AST-only | 0 |
| HIR-only | **55** (`dir_open`, `dir_close`, `dir_loop`, `syscall`, `to_c_string`, `dirent_c_string`, …) |

The assert at `src/backend/mod.rs:641` is

```rust
debug_assert_eq!(h.live, ast.live, "HIR/AST defn-liveness drift");
```

so `left` is **HIR** (79) and `right` is **AST** (24). `BUGS.md` (2026-11-16)
records "AST-only: 56, HIR-only: 0", i.e. the labels swapped. Consequence: HIR is
a **superset** here, not a subset — the shipped (release) set keeps *more*
functions live, and the set that loses callees is the **AST** analysis.

## 2. Root cause

Both builders reduce the program to one body per **bare name**, but they disagree
on which body wins a collision, and members share the bare-name slot with
top-level declarations:

`src/analysis/defn_liveness.rs`

```rust
fn index_defn(&mut self, d: &ast::Definition) {          // used for MEMBERS (:303)
    self.defns.entry(d.name.clone()).or_insert(&d.body);  // and EXPORTS (:394)
    self.contracts.entry(d.name.clone()).or_insert(&d.contract);
}

TopLevel::Definition(d) => {
    // Top-level: plain insert is the established last-wins
    self.defns.insert(d.name.clone(), &d.body);           // :377  LAST-WINS
```

`src/hir/defn_liveness.rs:190` uses last-wins `insert` **everywhere**, including
exported defns.

So when the prelude pulls in `std/collections.bv` (which declares
`obj HashMap { defn entries(…) }`, an early member) and `dir.bv` contributes
`export defn entries(…)` (later), the AST keeps the **HashMap member's** body
under the name `entries` and never walks dir.bv's — losing `dir_open`,
`dir_close`, `dir_loop` and their closures. HIR, being last-wins, keeps dir.bv's
body and finds them. Neither is right: **each drops one of the two bodies.**

## 3. Minimal repro (both directions fail)

```
obj Bag { defn entries(t: Int) [true][true] { term t; }; };
export defn entries(x: Int) [true][true] { term helper(); };
```

→ `helper` is **HIR-only** (HIR last-wins keeps the export; AST first-wins keeps
the member).

Reverse the declaration order → `helper` is **AST-only**.

That symmetry is the proof: this is an ordering artefact of a lossy index, not a
gap in either walker. It also means the drift is latent for *any* program where a
type member and a top-level/exported declaration share a name — `entries` is
merely where it became visible.

## 4. Why the parity gate I just added did not catch it

`tests/hir_defn_liveness_parity.rs::corpus_defn_liveness_parity` compares
`AstLiveness::build(&items)` against `HirLiveness::build(&lower_program(&items))`
on the **raw parse** of each file. The collision needs two declarations from
*different* files (one imported), so raw-parse comparison cannot see it — the
prelude is what introduces `HashMap.entries` alongside `dir.bv`'s `entries`.

It also `continue`s past files whose `unsupported_counts != 0`, so a lowering gap
would present as a silent skip. (Measured: `dir.bv`'s own unsupported count is 0,
so it *is* compared — it simply cannot trigger.)

## 5. Impact, corrected

- **Emitted code is unaffected**: HIR ships, and its set is the larger one.
- **The real risk is the AST consumers**: `src/backend/llvm/mod.rs:7618`
  (`build_with_roots` for closures), the reclamation/lifecycle analyses
  (`src/analysis/lifecycle.rs`, `src/analysis/reclamation/structured_tests.rs`)
  would treat dir.bv's helpers as dead while they are live.

## 6. Fix plan

**Primary: index a list of bodies per name in BOTH builders, and walk all of
them.** A name is live if any body is reachable; the live *set* stays name-keyed,
because every consumer asks "is defn X live?" and a name-level answer is the
conservative one (a member and a top-level with the same name keep each other
alive — safe direction, and already the status quo for whichever one happened to
win).

- `src/analysis/defn_liveness.rs`: `defns: HashMap<String, &[Statement]>` →
  `HashMap<String, Vec<&[Statement]>>`; `index_defn`/`index_txn`/exports push
  instead of `or_insert`/`insert`; the closure fixpoint walks every body for a
  reached name. Same for `contracts`.
- `src/hir/defn_liveness.rs`: mirror it.
- This removes the ordering dependence entirely, so the two agree by construction
  rather than by luck.

**Alternative considered and rejected for now:** type-qualify members
(`HashMap::entries`) in both, as `analysis::storage_class::index_type_members`
already does. It is more precise, but it requires member-aware call resolution
(a bare `entries()` inside a type body must bind to the receiver's member) and
changes the meaning of the live set for every consumer. Not worth it for a
conservative-superset bug.

**Residual, to document:** with a name-keyed set, a member and a same-named
top-level definition share liveness. That is over-approximation, not unsoundness.

## 7. Verification plan

1. Un-ignore `tests/hir_defn_liveness_parity.rs::dir_bv_as_root_does_not_trip_the_liveness_assert`
   — it asserts on the drift message in `brievc`'s stderr, so it pins the real
   path.
2. Add a curated parity case for the collision, **in both declaration orders** —
   the reversed order is what proves the fix is order-independent rather than
   "last-wins everywhere now".
3. Strengthen the corpus gate: give `corpus_defn_liveness_parity` a resolved-
   program variant (resolve imports for each file, compare, skip on unsupported
   nodes with a *counted* skip — report skips, never silent). Trade-off: this is
   the only variant that can see cross-file collisions, and it costs the resolver
   per file.
4. Docs: correct `BUGS.md`'s direction (AST loses, HIR keeps; shipped path is the
   conservative one) and replace "release ships the smaller set / live defns get
   non-live treatment" with the AST-consumer risk.

## 8. Open question for the fix

`backend/llvm/mod.rs:7618` uses `build_with_roots` on a *subset* program
(closures). Confirm after the change that the multimap index does not change
closure liveness — the fixture set will show it, but it is the one consumer whose
behaviour is not obviously preserved by "walk every body of a reached name".

## 9. Outcome (implemented 2026-11-18)

The fix landed as described in §6, with one narrowing found while reading the
code: the AST builder already had the collision-safe multimap (`member_bodies`,
added 2026-10-15 for member-vs-member collisions). So this was not "add a
multimap" but "register one more kind of body in the multimap that already
exists" — exported defn/txn bodies (`index_txn` was already unconditional on the
HIR side). Six lines per side.

Deviation from §7.3: the **resolved-program corpus variant was not added.** The
import-conflict rule rejects two modules providing the same name, so the only
realistic cross-file collision is member-vs-top-level — precisely what the new
two-order unit test pins. A resolver-per-file corpus run would add cost without
a class of coverage.

Verified: `dir_bv_as_root_does_not_trip_the_liveness_assert` un-ignored and
passing, the two-order symmetry test passing (both fail without the fix), lib
suite 3921, `daily_use_smoke` 46 golden tests unchanged.
