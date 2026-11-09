# Memory-defect campaign (use-after-free, leaks, region escapes)

## Goal
Aggressively hunt **memory defects in generated native code**: use-after-free,
double-free, invalid/overflowing access, uninitialized reads, and leaks. These
are the shapes the project keeps hitting (region reclamation freeing published
storage, sret images dangling after a defn returns, nested-object field-width
overwrites — see `BUGS.md`). Detection relies on sanitizers/Valgrind, not on
comparing outputs, so a defect surfaces even when the wrong answer is harmless.

## Why not cargo-fuzz here
`cargo-fuzz` runs the compiler **in-process** (and already under ASan), so it
cannot see memory bugs in *emitted* code. This campaign compiles real `.ll`,
links it under sanitizers, and runs it — the same pipeline that exposed the
region and boundary bugs historically.

## Detection stack
- **Valgrind** memcheck — primary access-check oracle (no IR instrumentation).
- **ASan via `clang -fsanitize=address`** — interceptor-based: leaks (LSan) and
  free-family errors. NOT access checks: clang does not instrument `.ll` inputs.
- Verified 2026-11-09: planted UAF caught by Valgrind; a generator bug (8-byte
  write into a `size`-byte alloc) was ALSO caught once Valgrind ran — the
  earlier ASan-only runs had been blind to it.

## Layers
1. **Corpus sweep** — run every memory-rich fixture under all profiles. Breadth
   over programs already known to be valid.
2. **Generated variants** — deterministic, seeded programs over the risky
   shapes: escape-from-callee, publish-into-parameter, list alias/mutate,
   defn-local image stored to node state, per-iteration allocation in a bounded
   loop, string build/discard. Bounded sizes/depths; each ends in `endprogram`
   so it terminates and is observable.
3. **Shrinking + replay** — Proptest shrink on failure; persist a JSON case +
   the generated `.bv` + sanitizer stderr under an artifact directory.

## Aggressiveness controls
`MEM_CASES` (count), `MEM_SEED` (campaign), `MEM_FAMILY` (fixed shape),
`MEM_SHARD=i/n` (disjoint case ranges), `MEM_LEVELS` (opt/san levels),
`MEM_VALGRIND=1` (enable memcheck), `MEM_ARTIFACTS` (output dir). Nightly CI
runs a sharded matrix; a bounded PR smoke runs the corpus sweep only.

## Bounds and honesty
- Sizes/depths/iterations are bounded so a report is actionable, not a runaway.
- `timeout` contains hangs; a hang is infrastructure, not proof of nontermination.
- A sanitizer finding is a **defect candidate**; it is confirmed by minimizing
  and re-running before filing. Leak reports on `Malloc#`-by-design ownership
  must be checked against the ownership contract, not filed blindly.
- Independent of `deterministic_transitions` (which checks *values*); this
  checks *memory safety*. Both are needed.

## Verification
Localized per-commit: focused sweep + generator over a few families, demo of a
planted fault being caught (synthetic, opt-in) or a real finding. Library suite,
build, Praetor on changed dirs, YAML validity for the CI file. No production
compiler change; no performance claim.

## Results
- **One confirmed defect** (memory leak): publish-into-incoming-parameter.
  Family 1 fails 12/12 across 3 seeds; minimal repro
  `tests/fixtures/memory/publish_into_parameter_leak.bv` → **16 bytes definitely
  lost** (Valgrind). Control (assign directly in the caller) is clean. Root
  cause `src/analysis/reclamation/walk.rs:354` (barrier disables reclamation).
  Corroborated by the standing red guard
  `reclamation_runtime::native_aliases_borrowed_payloads_and_nested_returns_are_memory_safe`.
  Logged in `BUGS.md` (OPEN).
- **Methodology defect fixed:** ASan on `.ll` does not instrument accesses;
  the first harness was blind to UAF/overflow and to a generator bug. Valgrind
  is now the primary oracle; a planted UAF is caught.
- Clean: corpus sweep; families 0/2/3/4.
- Family 1 is a standing finding: out of the default family, nightly with
  `continue-on-error`.
- Full report: `docs/reports/2026-11-09-memory-defect-campaign.md`.
