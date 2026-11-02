# Language stabilization — 2026-11-02

**Status:** active. `main` is `d6d32c05`, all contributor PRs closed, no open
PRs. This plan is a bug-ledger re-triage and core-correctness queue.

## Rule

Performance work is frozen until Stage 0 and Stage 1 complete. Missing proof may
cost performance, never correctness. No OPEN core bug is waived because the
current app gates pass.

## Stage 0 — Re-triage every OPEN core-language entry — COMPLETE (2026-11-02)

For each known repro: run it on current `main`, record FIXED / STILL OPEN /
SUPERSEDED, and update `BUGS.md`.

Priority ledger:

1. `List<Result>` elements alias later enum constructors.
2. JsonValue object nested in Array loses payload.
3. `list_concat` intrinsic has no lowering.
4. Char cast lane mis-typed.
5. Interpreter callable txns are not executed.
6. Interpreter raw-address→String unsupported.
7. Interpreter large `ws_encode` reactor path.
8. Generic callable txn specialization deferred.
9. C-driven bridge/CLI tests quarantined. (Bridge family fixed; CLI still open.)

Result: `List<Result>`, JsonValue nested array, `list_concat`, Char cast,
interpreter callable txn, raw-address→String, and 7000-byte `ws_encode` are
fixed/verified. C boundary, needs-state, and CLI-entry tests still fail/hang.
No ambiguity remains in those entries.

## Stage 1 — Core ownership/correctness fixes — COMPLETE (2026-11-02)

1. Collection elements own a complete enum value or explicit box; never a
   borrowed constructor image. Unignore
   `tests/enum_list_lifetime_regression.rs`.
2. Fix JsonValue nested object payload lifetime if still present.
3. Implement `list_concat`.
4. Fix Char protocol/cast width.
5. Add regressions and run enum/collection/JSON/ws gates.

Exit: no OPEN value/collection correctness bug.

## Stage 2 — Reference interpreter parity — COMPLETE (2026-11-02)

1. Execute callable `txn` bodies like `defn` calls.
2. Support raw-address→String construction.
3. Fix large `ws_encode` reactor path.
4. Add interpreter/backend differential regressions.

Exit: Rule-5 parity for callable txns and string construction.

## Stage 3 — Test/language hygiene — IN PROGRESS (2026-11-02)

1. Generic callable txn specialization.
2. Unquarantine C bridge/CLI tests.
3. Make `cargo test --all` a required release gate.
4. Add conformance coverage for every fixed bug.

Progress 2026-11-02: C boundary (`greet`/`join`), `needs_state` (31/31),
C++ roundtrip, Python native extension, and Python<->Node bridge are fixed and
unignored. CLI entry (`entry_cmd_greet_quiet`/`_verbose_name`) is fixed:
the entry plugin's done flag was emitted after a trailing bare `term;`, which
branched past it. Root cause for the bridge family: `CStr` is a raw pointer at
the boundary; casting through `String` parsed the allocation header. Remaining:
generic callable txn specialization and Go roundtrip (Go toolchain absent).

## Stage 4 — Platform ledger

Triage GPU OPEN/UNVERIFIED entries and Web/wasm issues; close only with
reproducible evidence.

## Stage 5 — Release stabilization gate

- `cargo test --lib`
- `cargo test --all`
- daily suite
- determinism
- full runtime
- GLUE/bridge, wasm, Node/JS, I11
- no OPEN core correctness bugs
- final known-limitations document

## Local branch hygiene

Open PRs: 0. Local experiment branches predate this queue and are not current PR
work. Do not merge them blindly. Archive/prune only after owner review.
