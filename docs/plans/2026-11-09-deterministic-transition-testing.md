# Deterministic transition testing

## Goal and scope
Add an executable, bounded differential campaign using existing Proptest support.
Generate integer transition functions and event scripts; compare interpreter
results with real LLVM output linked at O0 and O3. Runtime environment inputs
prevent the compiled transition from being folded to a constant.

## Evidence and implementation
Existing `tests/mixed_integer_operands.rs` demonstrates build/link execution;
`tests/dyn_list_interpreter_parity.rs` demonstrates interpreter setup. Existing
frontend fuzzing tests robustness rather than native semantic agreement.

Implement generation, replay bundles, property shrinking, and explicit synthetic
observation corruption to verify detection. This is a transition simulator, not
a scheduler or virtual-I/O implementation. Arithmetic is bounded to avoid overflow.
No compiler optimization changes or performance claims; benchmark timing is not
part of this correctness-only work.

## Verification and documentation
Run the focused campaign, synthetic-failure/shrink/replay experiment, library
suite, build, Praetor on tests, and quick IR determinism gate. Document commands,
scope, artifact handling, and CI usage in a new guide. Preserve existing comments
and source files; no architecture/API changes are planned.

## Execution evidence
- Four generated cases passed against O0/O3 native execution.
- Synthetic observation corruption failed and shrank to initial=0, threshold=0,
  bias=0, events=[0], with interpreter=[0] and native=[1] (146.60 seconds).
- Saved-case replay passed without corruption and failed with corruption.
- An independent reference assertion covers guard equality, positive/negative
  events, bias, and event ordering. The final focused suite passed both tests.
- `cargo build` passed without warnings; Praetor validation of `tests/` passed.
- Full `cargo test --lib` passed: 3553 tests, zero failures (338.73 seconds).
- Release compiler build passed; the subsequent quick IR determinism gate
  passed all three corpora (recommender, multinode, pp-types).
- The first quick determinism invocation could not run: no release compiler
  existed. A release build and the full library suite were launched separately.
- Existing CI already runs the integration test through `cargo test --all`;
  an always-run upload step preserves failure artifacts.

No compiler defect was established by this bounded campaign. The demonstration
fault is explicitly synthetic and does not warrant a BUGS.md defect entry.
