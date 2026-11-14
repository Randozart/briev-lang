# Post-re-land: hygiene, soak truth, tail-loop destruction (2026-11-14)

## Context

Census campaign 2 is re-landed (intel `80abfc32c`, includes PR #96):
undecided 50 → 12, releases 58, ratchet live in kiosk acceptance
(`harness/memcheck_ratchet.sh`, baseline 58/12, CI step green). The two
blocking bugs are fixed and documented in
`2026-11-14-census-reland-and-ratchet.md` (coll-object owns-check, self-tail
temp-transfer exclusion).

Known debts this leg closes:
- kiosk pin is one merge behind (`5ff1fec19` vs intel `80abfc32c`, PR #96
  touches portable storage/ownership — must re-gate kiosk against it);
- soak regressed from flat 177 MB to a 159→188 MB stair-step @300 (HWM 244);
- the self-tail exclusion is a LEAK with a main-lane test documenting the
  intended behavior (`tests/tier1/owned_tail_cleanup.bv`: "ownership cleanup
  after a recursive result must run after the call returns" — it passes only
  because the golden checks lengths, not memory);
- kiosk local `wip/census-campaign-2` branch swept in scratch files.

## A. Hygiene + integration

1. Bump kiosk `compiler.lock` to intel `80abfc32c`; re-run: native smoke,
   ML smoke, VAD RPC, VAD sessions, dashboard rbv IR verify, memcheck census
   (update ratchet baseline deliberately if it moves); push.
2. Reconcile `main`: currently blocked by the other agent's uncommitted
   backend edits (fuzz/Cargo.lock, emit_toplevel.rs, backend/mod.rs).
   Intel is the source of truth; poll and FF when their worktree is clean.
   Do not touch their work.
3. Kiosk WIP cleanup: `git switch wip/census-campaign-2`, remove
   `build-vadsess/` and `okprobe.bv` from the commit (unpushed; reset+recommit
   is fine), keep daemon.bv if it still differs from main; then delete the
   branch's scratch or leave it clean.

## B. Memory program completion

### B1. Soak truth (do first — measurements gate the rest)

Goal: explain 159→188 MB @300 (stair-step, ~3 steps of +9 MB) vs flat 177.
Hypotheses, tested in order:
- coll-object owns-check retains objects: objects allocated via `Alloc#`
  inside the *entry region* report `coll_arena_owns == 0` → freed normally;
  objects from the request block are skipped (correct). Instrument with a
  C-side counter (alloc vs owns-skip) in `coll_arena.c`, print at shutdown.
- freed-object churn: object free/realloc against glibc arenas per request.
- allocator staging (previous runs staged too): compare 500-call curve; a
  true leak grows linearly, staging steps then flattens.
Acceptance: either flat-or-explained curve with numbers in the results
record; no unexplained growth.

### B2. Tail-loop-aware destruction (replaces the self-tail exclusion)

`peel`/`strip_frac_zeros` shape: `term self(transformed_arg)` where the arg
is an OWNED temporary (slice/copy). Tail-loop lowering turns the call into
`br .tail_header`; the loop arg slot is rebound each iteration. Correct
cleanup:
- release the PREVIOUS iteration's owned arg right after the header rebind
  (in the loop body, before the next use), and
- release the FINAL owned arg at loop exit (the base return path).
Requires the tail-loop lowerer to accept a per-arg "owned" flag from the
owned-storage pass (frontend fact consumed by backend, per doctrine), not a
name/type heuristic. Delete the `self_tail` exclusion in `owned_storage.rs`
with a provenance note; turn `owned_tail_cleanup.bv` into a memory-checked
gate (or add a malloc counter assertion).

### B3. Census 12 → 0, then deny-at-zero

Remaining families: parser attr fallbacks (`packed_ints`/`conv_attr4`/
`attr_ints`/`axes_of` and the parse_*_loop chain — `fresh_list` copies on
the param-return bases), `arg` literal (`fresh_string("")`), `json_get` /
`resolve_str_val` copies, `select_path`/`dispatch` residuals. Each fix is
the §13 copy discipline; re-census after each batch; ratchet baseline
updated downward deliberately. When 0: switch kiosk CI to
`--deny-memory-ambiguity` and keep the ratchet as belt-and-braces.

## C/D. Deferred (need owner decisions)

- STT/TTS/visemes native (whisper artifact + tokenizer; TTS/viseme model
  choice); NPU lane priority. Reference surface is 7 steps; native has 6
  methods.
- PTL runner network drops (~15 min tailscale/disco churn; needs sudo or a
  host-side selfdiff timer). Functional gates proven in clean runs; oracle
  verified manually (MATCH 7 steps).

## Gates (every commit)

`cargo test --lib` + conformance; kiosk native/ML/VAD RPC/VAD sessions/
dashboard; census + ratchet; 300-call soak recorded in the results file with
same-geometry correctness (decode probe or face count) — timing/memory
numbers only with the gate.

## Open decisions

- B3 target: deny-at-zero now, or stay on ratchet and move to C after B1/B2.
  (Recommended: finish B3 — it is copy discipline with a clear end.)
