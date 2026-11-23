# Worktree & branch hygiene record (2026-11-22, wave-10 Stage A2)

## Removed

- Worktrees: briev-basetest, briev-assurance-main-validation,
  briev-lean-assurance, briev-g4, briev-e14a, briev-proof-cache,
  briev-phase-c4, briv-compiler-bad-dialect, briev-dogfood,
  /scratch/briev-builds/{counsel-seed,evented-net}, pr146-toolchain
  (pruned — gitdir pointed nowhere).
- Branches merged into main (57): backend-foundation, bits-strings,
  compiler-in-brief, docs-ff, experiment/effect-carry-selection,
  feat/accel-gpu, feat/bad-dialect, feat/compiler-assurance-lean,
  feat/counsel-expand-final, feat/counsel-seed, feat/data-brief,
  feat/derivation-synthesis, feat/destination-ownership-d7-d8,
  feat/e14a-intent-synthesis, feat/effect-shape-followups,
  feat/evented-net, feat/fullstack-task-board,
  feat/generalized-value-destination-ownership, feat/knooppunt-dogfood,
  feat/local-effect-canonical, feat/nested-carrier-slots,
  feat/nested-enum-layout, feat/net-socket-tcp, feat/out-observability,
  feat/proof-result-cache, feat/selective-copy-elision,
  feat/small-sum-unboxing, feat/spec-layout-keywords,
  feat/tail-recursion-lowering, feat/typed-hir,
  fix/closed-region-ownership-port, fix/divergence-willreturn,
  fix/method-call-explicit-receiver, fix/wasm-generic-collections,
  local-main-snapshot, phase8-pp-port-2026-07-22, pr-52/53, pr79-93,
  pr91-review, wasm-webstack-v2, wave4-trunk-repair, wave5-docs,
  wave5-stdlib, wip/census-campaign-2, feat/stdlib-path-check (content on
  main at src/pipeline.rs:986), feat/verify-summaries (content on main at
  briev-verify/src/symbolic.rs:344).
- Patch-id-verified duplicates: pr125, pr126, pr127, main-prep, val92,
  val93, feat/tail-loop-lowering, feat/counsel-contracts, feat/cli-help,
  electronics-hir-port2.
- Stashes created: dogfood tree, counsel-seed tree (labelled, recoverable).

## Kept (each with a reason)

- briev-compiler-baseline worktree (locked A/B baseline, AGENTS.md;
  .parked-untracked-aug/ is deliberate).
- k3-baseline-trunk worktree (a session's active tree, files touched today).
- briev-intel-xpu worktree (lane session's tree, clean).
- briev-call-fact-soundness / fix/call-contract-fact-scope (8 commits of
  Kani repro work; relevant to the next fact-engine campaign).
- feat/counsel-lab (51 commits: counsel roadmap + plans; candidate for a
  separate harvest decision).
- recovery-branch (35 commits, old-era experiment; kept labelled).
- feat/verify-mask-offset-kernel (9 commits, PR #104-era kernel work).
- h5-phase-c4 (6 commits, unmerged audit).
- fix/aggregate-arg-handle-abi (5 commits).
- feat/canonical-effect-abi (6 commits; carries its own abandon doc).
- feat/counsel-expand (5), backup/pre-a2-cleanup (5),
  feat/stdlib-checksum (2), feat/counsel-seed-v2/v3 (superseded content),
  feat/counsel-merge-test (7), experiment/* (labelled as such), wave6
  (this wave's tree), feat/evented-net (PR #147's branch).
- main folder: on main at origin tip; the four untracked .opencode research
  notes are now committed (goose, stdlib-critique) or archived
  (archive-opencode/).

Result: 16 worktrees -> 7; 96 branches -> 33; zero content destroyed that
was not verified present on main (merge-ancestor check or patch-id check)
or preserved via stash/copy.
