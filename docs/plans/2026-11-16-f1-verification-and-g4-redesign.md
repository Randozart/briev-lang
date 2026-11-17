# F1 verification + G4 redesigned, then the rest of the grind

**2026-11-16.** Status: **plan — research complete.** Supersedes two claims I made
earlier today; both corrections are recorded here with their evidence.

**Parents:** `2026-11-16-ffi-remaining-grind.md` (§4 G4),
`2026-11-16-ffi-followups.md` (F1),
`2026-11-16-s3-protocol-generation.md` (§G4 blocked-by-F1 note).

---

## 1. Two corrections

### 1.1 F1 is already fixed — it was not what I measured

`BUGS.md:112` ("emitted IR is not reproducible") records three causes and their
repair: three unsorted `owned.keys()` iterations in `src/hir/owned_storage.rs`, then
"**Second fix (2026-11-16, M6) — the state-index drift, and the daemon now differs
by zero bytes**". The remaining diff was `%ip_N`, which the same entry explains is
*not* a second bug: it is the init-store register for `%State` field `N`, so it
renames whenever an index shifts.

So the per-process instability this session's follow-up plan ranked as top risk has
**already been diagnosed and repaired**. F1 is therefore not a fix task but a
**verification-and-gate-widening** task: prove the repair holds on more than one
program, and make the gate able to catch a regression.

### 1.2 My G4 reproduction was not a determinism failure — the gate was mis-designed

I diffed `cstr_str_to_c_meld.ll` before and after splitting `lib/glue/c.bv` so that
it *imports* `protocols.bv` instead of declaring the protocol inline, and read the
3471-line difference as F1. It is not:

- `collect_strings` (`src/backend/llvm/mod.rs:880`) walks `items` **in order** into a
  `Vec`, deduping with a `HashSet` used only for membership. Interning order is
  deterministic *given* the item order.
- The item order changes because the **program structure changed** — the protocol
  now arrives from a separate imported module rather than inline in `c.bv`. Strings
  interned earlier (from the newly imported module's items) take lower numbers.

Measured evidence: the interned-string **content set is identical** (`SAME STRING
SET` after normalising `@str.N`); only numbering and declaration order move.

**Consequence: "byte-identical IR between an inline declaration and an imported
one" is not a determinism property and must not be demanded.** It asks the emitter
to be invariant under program restructuring, which it has no reason to be and
should not be — item order is a legitimate input.

---

## 2. What G4 should be instead

Keep the strongest possible claim, but make the two variants *structurally identical*:

> Compile a fixture twice, where **both** variants import a protocols file at the
> same place in the same module; the only difference is whether that file's contents
> were written by hand or produced by `brievc render`. Require **byte-identical
> IR**.

That isolates exactly the claim worth proving — "the generator's output is the same
program as the hand-written one" — without asserting invariance the emitter does not
owe. It stays a byte gate: no normalising, no tolerance, nothing that could mask a
real difference.

Method:

1. Create `tests/fixtures/glue_gen/baseline/protocols.bv` (hand-written copy of
   today's generated content) and keep the generated one at
   `lib/glue/c/protocols.bv`.
2. The parity test builds the fixture twice with a `--glue-protocols=<path>`
   override (or by swapping the imported path in a temp copy of the fixture), then
   compares bytes.
3. Corpus must cover **both directions** — Briev→C (`c_strlen` on a `CStr`) and
   C→Briev (a returned `CStr` consumed as a String) — because an inverted
   `#Lh`/`#Rh` shows in only one.

---

## 3. F1 verification + gate widening (small, do first)

`benchmarks/ll_determinism_check.sh` compiles its corpus twice in separate processes
and byte-compares. It passed while this bug existed because **none of its programs
exercised the combination** (state-field count × interned strings × owned-storage
destruction order).

Widen it with three properties:

1. **Add the recorded repro** — `briev-kiosk-avatar/briev/service/daemon.bv` if it is
   reachable from this tree, otherwise a local program with the same shape (many
   fields, `coll_destroy` sites, interned literals).
2. **Vary the process, not just the run** — already done; keep 2 runs minimum, 3 for
   the widened corpus.
3. **Vary the import graph** — compile the same logical program with a two-level and
   a three-level import nesting; both must be internally deterministic (each variant
   compared only against itself, *not* against the other — see §1.2).

Acceptance: the widened gate would have failed on the pre-fix commit. Verify by
running it against the fix's parent (a worktree checkout, not a revert) before
declaring it done.

---

## 4. Then the grind resumes

Order unchanged from `2026-11-16-ffi-remaining-grind.md` §3, with G4's design
replaced per §2:

| Step | Content | Gate |
|---|---|---|
| **F1v** | verify the repair + widen the determinism gate (§3) | widened gate green now, red on the pre-fix commit |
| **G4′** | parity gate with structurally identical variants (§2) | byte-identical IR over a both-directions corpus |
| **G5** | `glue/c/types.bv` — generate from `values:` or delete the dangling key | no dangling `types_module` |
| **H1–H3** | `HirProtocol`, `HirBoundary`, audit reports missing facts | the BUGS boundary fixture stops auditing `unsupported=0` *because facts are modelled* |
| **H4** | destination model reads `HirBoundary` | classifications unchanged (existing tests are the oracle) |
| **S5** | stage guard: declaration synthesis only at `Resolved` | compile-fail fixtures both ways |
| **PY** | Python as pure data | no Rust change; C path byte-identical |
| **R1** | delete the hand-written protocol half | G4′ still green |
| **D1–D2** | removed-surface gate, then sweep | gate green; no live file uses a removed form |

---

## 5. New findings worth keeping

- **A "same source" parity claim must pin the *structure*, not just the text.**
  Splitting a declaration into an imported module is a different program to the
  emitter. This will recur for `types.bv`, for the Python adapter, and anywhere else
  a file is split — so the rule belongs in G4′'s doc comment.
- **`collect_strings` is deterministic and should stay that way.** It is a `Vec` +
  membership `HashSet` — the right shape. Any future edit that iterates a hash
  container there would reintroduce F1; a comment saying so is cheap insurance.
- **The determinism gate's corpus is the blind spot, not the gate.** Three programs
  passed while the bug shipped. Widening the corpus is the fix, and the recorded
  repro is the first addition.

## 6. Documentation to update with the code

| Doc | Change |
|---|---|
| `2026-11-16-s3-protocol-generation.md` | replace the "G4 blocked by F1" note with §1.2's correction and G4′'s design |
| `2026-11-16-ffi-followups.md` | F1 becomes "verify + widen the gate" (the fix already landed); re-rank the remaining items |
| `BUGS.md:112` | append that F1 is verified by the widened gate, and that the import-graph difference is *expected*, not a leftover |
| `docs/architecture/backend-contracts.md` | the rule from §5: a parity claim pins structure; item order is an input, not a bug |
