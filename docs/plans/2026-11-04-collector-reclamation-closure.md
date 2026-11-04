# Collector reclamation and expansion closure

Date: 2026-11-04

## Scope and existing plans

Application delivery plan: `C:/Users/wotto/code/recommender-app/docs/plans/2026-11-03-briev-native-load-harness.md`.
Leak evidence: `.kiro/specs/recommender-app/memleak.md` and the November 3/4 collector investigations. Preserve these historical records.

This is a memory-correctness repair; no throughput claim or performance optimization is planned. The existing controlled IR experiments establish independent container, payload, parser and observation leaks. They do not establish general ownership safety.

## Implementation order and proof gates

1. Resolve nested environment expansion with the exhaustive mutable AST visitor. The partial environment walker omits foreach bodies, guards, index operands, match scrutinees, and nested intercept arguments. Preserve compile-time environment constants and receiver-first call semantics. Add behavior tests for nested and wrapped contexts, unknown plugins and constant behavior.
2. Trace fresh allocation identities and borrowed/static provenance independently from destination representation. Propagate return summaries to a fixed point across recursive calls. Track projections, aliases, active enum variants, replaced backing allocations and ownership transfers. Unknown provenance cannot authorize release.
3. Derive release sites after all aliases' uses, accounting for branches, early exits, loops and escaping storage. Consume frontend facts in LLVM without function/type-name recognition. Caller destinations and borrowed payloads must never be released.
4. Run isolated allocation gates before collector validation: success/growth/open/read errors; escaping and late-used aliases; static/borrowed values; nested enums; caller destinations; recursion and early exits. Require zero invalid accesses and definite/indirect leaks under Valgrind or ASan.
5. Run the application memory gate and 5,000-sample collector preflight. Do not claim load-test readiness while any memory gate fails.

## Documentation and verification

Preserve existing rationale comments and unrelated working edits. Document provenance and release consumers in `docs/architecture/llvm-memory-management.md` and the destination ownership ADR. Record each proven defect and repair in `BUGS.md`. Explain generic Briev examples in any upstream PR.

For expansion, document the shared visitor contract in compiler architecture documentation. Run focused regressions, cargo test --lib, cargo build, Praetor on changed directories, native/wasm integration and relevant determinism checks before a targeted commit. Record unavailable gates explicitly. A full runtime baseline is required before any subsequent performance work; none has been measured here.

## Workflow dependency

The workflow is `.kiro/specs/recommender-app/WORKFLOW.md` (uppercase). Read after correcting the initial case-sensitive filename search. Follow its small-increment teaching cadence, Windows source/Linux cache split, final-suite timing and approval-before-commit requirement.

## Expansion increment verification

Three focused environment-plugin unit tests pass. The native integration test
passes and prints `nested_env_ok`, checking runtime loop bounds, a guard and a
nested environment lookup. `cargo build` passes without new warnings. Final
library verification passes 3,441 tests, zero failures, with only the previously
delegated conformance sweep excluded. Select the installed current SPIR-V
validator and LLVM 17 tools on PATH; the system validator is too old for the
suite and bare opt is absent from the default PATH. Quick IR determinism passes
all three corpus programs. Praetor passes tests/ and reports no diagnostics in
the changed environment plugin; its 17 plugin-directory diagnostics concern
untouched files. Initial fixture syntax and tool selection failures were
corrected before these final verdicts.

No automatic reclamation has been implemented in this increment. File-result,
parser and observation leaks, alias-safety gates and collector memory/5,000-row
validation remain open. The workflow requires approval before committing the
completed logical increment; no commit or push has been made.

## Region derivation implementation (memory repair, in progress)

Use an allocation-identity graph with copy, projection, memory-store,
return and call-argument constraints. Solve recursive edges to a fixed point.
A function region is licensed only when none of its transitive allocations
reaches its return, incoming storage or an unclassified consumer. Globally
escaping allocation owners suspend region inheritance; unknown callees also
block proof. Caller result images are not allocations in the runtime registry.
Allocation replacement and explicit release update private records, preserving
the public malloc pointer ABI. Active-region lookup remains available while
tracking is suspended, so an explicit release can remove an older record.

Required negative gates include pointer-bearing Int returns, late-used aliases,
parameter field retention, static/borrowed enum payloads, unknown callbacks,
custom allocator paths and raw-memory pointer stores. The native consumer must
invalidate stale memory-effect attributes and preserve control-flow dominance.
This remains implementation work until these safety gates and the original
collector memory gate pass. No performance numbers or bounded-memory claim.

## Generated-IR experiment and completed implementation

The first call-region consumer eliminated the heap leaks but inserted TLS-visible
frames into the recursive sample driver through inlining. The resulting linked
binary retained one stack frame per iteration. Test the hypothesis before changing
the consumer: start with the actual emitted collector IR, give definitions that
contain a region frame `noinline`, then link control and experiment with:

```sh
clang -O3 -flto -march=native -ffast-math -fdata-sections -ffunction-sections \
  collector.ll lib/runtime/briev_rt.c -o collector
```

Both arms emitted 5,000 successful rows at 500 Hz. Control RSS grew from 250 to
794 pages, with virtual memory growing from 2,580,480 to 4,530,176 bytes after
warmup. Experiment RSS went from 254 to 286 pages; virtual memory stayed at
2,580,480 bytes. These are memory-correctness observations, not throughput data.
The emitted-IR experiment files are under `target/reclamation-runtime-check/`;
raw rows are `/tmp/briev-region-ir-control.csv` and
`/tmp/briev-region-noinline.csv` in WSL.

The consumer now retains proved nonrecursive scopes as separate calls. Recursive
functions inherit enclosing regions; proving a recursive iteration consumer is
required before adding their own exit cleanup. A second defect prevented the
first compiler-generated verification from reproducing the experiment: group 15
was referenced but never emitted, and cloned groups reused its ID. Emit the
conservative group and reserve all references, including undefined ones. Tests
pin quoted strings, pure functions and undefined references independently.

The frontend also distinguishes uncertain numbers from exact constants. A
dynamic syscall number with a literal operand must not inherit that literal's
borrowing effect. Unit coverage checks computed and partially unknown numbers.
Entry proof selection uses the sole reactive node, regardless of its name.

Kani 0.68 was installed and setup completed. Its bundled executable cannot start
on this WSL Ubuntu 20.04 glibc: it requires GLIBC_2.32, 2.33 and 2.34. The two
new alias-escape harnesses therefore remain unverified by Kani. Native allocator
failure and alias behavior are checked under Valgrind; this is not a substitute
claim of formal verification. See `/tmp/briev-reclamation-kani.log`.

Final proof review added a conservative entry-initializer gate: executable or
allocating globals cannot silently enter a scope whose proof covers only the
node body. Include reactor conditions in that body graph as well. Independently
proved helper regions remain available when process-entry eligibility is absent.
Negative tests cover global initialization and unknown reactor conditions.

The wasm regression gate exposed a separate allocator-width defect:
`Malloc#` emitted an i64 handle but returned a pointer type resolved to i32.
This produced invalid `zext i32` instructions consuming an i64 SSA value.
Share hosted allocation emission/provenance with `Alloc#`, already target-width
aware, and keep embedded handles/remaining size consistent with their target.
The existing per-instantiation wasm collection fixture exercises this path.

The expanded wasm corpus then reached a non-scalar pointer store that inferred
physical width from a one-letter generic type name. Member argument coercion
had actually widened that value, so the emitted `store i32` consumed i64.
Remove this spelling heuristic: use recorded register representation with the
casting graph's resolved type as fallback. Preserve target-width backing slots.
This is a representation-correctness fix, without algorithm/type recognition.

## Final collector memory evidence

Final compiler SHA-256:
`5a1025d902a5ba719ef74ad502a6be0c5b1c20411867660763657aa14279dffe`.
Collector SHA-256:
`285f48f0c08b327b499bacd0ee7027300e3992db856da8f7fbbeaeda375eb37a`.
Sources are the current Windows checkout; builds use LLVM 17 and the Linux
Cargo cache `/home/otto/briev-target`. No restart or source-level free was added
to the collector. All nine `check-memory.sh` cases pass with zero definite or
indirect loss and zero Valgrind errors. The remaining reachable allocation is
the process-lifetime 65,536-byte stdout buffer.

| Self-preflight | Successful rows | RSS first → last, pages | RSS after sample 1,000 | Virtual bytes after sample 1,000 |
| --- | ---: | ---: | ---: | ---: |
| 500 Hz, run 1 | 5,000 | 323 → 355 | 355 constant | 3,989,504 constant |
| 500 Hz, run 2 | 5,000 | 346 → 378 | 378 constant | 3,989,504 constant |

Require exactly 5,001 CSV lines including header, 19 fields per line, sequential
sample indices, error=0, stable PID/start identity, ordered deadline/read-bracket
times, 4,096-byte pages and 100 clock ticks/second. Both runs satisfy these
checks. Final raw CSV and summary are in the app's
`target/load-memory/preflight-5000-{1,2}.csv` and `preflight-summary.json`.

Reproduction from the app checkout in WSL:

```sh
export PATH=/usr/lib/llvm-17/bin:/home/otto/.cargo/bin:/usr/local/bin:/usr/bin:/bin
export BRIEV_ROOT=/mnt/c/Users/wotto/code/briev-lang
export BRIEVC=/home/otto/briev-target/debug/brievc
bash benchmarks/load/check-memory.sh
LOAD_PID=0 LOAD_HZ=500 LOAD_SAMPLES=5000 target/load-memory/collect > preflight.csv
```

Repeat the last command independently and validate both outputs. The app's full
`benchmarks/load/check.sh` also passes scheduler arithmetic/error cases, attempt
accounting, process sampling and collector schema/units/times/failed-read status.
These results close collector memory correctness, without claiming backend
capacity, network-user completion or a sustained application soak.

## Final regression verdict

- `cargo test --lib -- --skip conformance_sweep_every_active_source_parses_and_checks`:
  **3,459 passed, zero failed, one filtered** (the previously delegated source
  conformance sweep). A too-broad initial `--skip conformance` was corrected;
  the six lightweight conformance tests are included in this final verdict.
- `cargo build`: passes without new warnings. Final build SHA-256 is
  `d2582dd65100673fd48f4f6c976a7e6dfb7668356b939a444fac162f522875ff`.
  The final collector remains byte-identical to the hash in the memory table.
- Native integration: all three reclamation tests, nested environment,
  discarded enum payload allocation and integer output gates pass. Independent
  runtime checks use O0 and O3/LTO plus Valgrind and failure injection.
- Wasm: object-layout gate passes; collection gate passes the full int/struct/
  mixed-list, JSON, scalar widths, portable web, tuple and generic-scan corpus.
- Full `ll_determinism_check.sh`: seven programs pass byte comparison across
  separate processes. Toolchain matrix: installed LLVM 17 recommender row passes;
  multinode/giantnode runtime rows are explicitly skipped without `u.data`.
- Praetor was run on changed analysis/backend/plugin/test directories. Added
  reclamation and shared-specialization modules have no diagnostics; tests pass.
  Existing large compiler functions retain baseline diagnostics. `git diff --check`
  passes. Kani remains unavailable for the glibc reason recorded above.

An initial final-suite failure used the old system SPIR-V disassembler; selecting
the complete installed current SPIR-V tool directory corrected it. Literal IR
tests that demanded direct malloc/group numbers were updated to preserve their
heap/effect contracts when ownership bookkeeping wraps those instructions.

Memory repair is complete in the working checkout. No commit or push has been
made: the workflow requires human approval before committing. The root PR draft
explains the generic Briev construct and dependencies; an isolated upstream
carve-out must preserve unrelated working changes and prerequisite fixes.
