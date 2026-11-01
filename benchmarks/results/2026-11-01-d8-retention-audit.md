# D8 retention-helper audit — 2026-11-01

## `own_enum_value`

Probe: make the helper a no-op, run full gates.

- `cargo test --lib`: PASS.
- `cargo test --test daily_use_smoke`: **FAIL — 2 gates**.

Conclusion: still carries a unique object-field lifetime obligation. Retain.

## `retain_enum_return`

Probe: make the helper a no-op.

- Daily-use suite: PASS.
- Full runtime suite: all outputs MATCH; `ws_frame` improves to 0.36x C.
- `net_stack_helpers`: ASan and O3 PASS.
- Real recommender I11: **FAIL** — server never becomes ready; owns the
  historical server regression.

Restored helper; I11 immediately PASS:
`maxLongTaskMs=0`, `resultBeforeDatabaseCommit=true`,
`savedAfterCommit=true`, `recommendationRenderUnchangedAfterSave=true`.

Conclusion: still required at the return/ABI edge. The apparent ws_frame win is
not a valid removal — it trades a server correctness failure for speed.

## D8 consequence

Do not delete either helper until a proof-carrying replacement exists. D8 also
remains blocked on P2’s generated-foreach fallback and the D4b re-enable
decision.
