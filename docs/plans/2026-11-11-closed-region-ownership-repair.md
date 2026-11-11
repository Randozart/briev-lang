# Closed-region ownership repair

PR #90 is closed at the user's request. Its isolated catalogue memory gate
passed, but its authenticated-server integration aborted with a double free;
clean upstream `109e8bb4` passed the identical journey. The earlier publication
was premature. The interrupted Valgrind run did not establish the root cause.

## Branch and evidence

Repair branch `fix/closed-region-ownership` starts at upstream `c7a06444` and
replays the original candidate as `f63e7c56`. The only replay conflict was the
BUGS ledger; both entries were retained. This is not a verified replacement.
Original reproduction artifacts will live under the application's
`target/pr90-repair/`, rather than ephemeral `/tmp` storage.

## Gates and sequence

1. Rebuild the original candidate server, capture the native crash stack and
   allocation/free provenance; reduce to a self-contained ownership regression.
2. Trace proof roots, implicit helpers, nested scope ownership and runtime
   bookkeeping. Test the causal hypothesis before changing implementation.
3. Repair the general ownership rule, with negative escape and native O0/O3
   regressions plus Kani coverage for safety-critical new logic.
4. Verify the full authenticated journey and independent SQL; repeat catalogue
   1/4/16 memory checks using freshly hashed artifacts. No capacity claim.
5. Run library/runtime gates, warning-free build, Praetor, full IR determinism
   and toolchain matrix. Resolve remaining upstream conflicts before submission.
6. Update `BUGS.md`, `docs/architecture/llvm-memory-management.md`, ownership
   proof/runtime rationale comments and a new evidence record. Preserve old
   rationale by explaining corrected invariants; historical result records are
   not rewritten. Submit only the verified fix branch and integrate it into a
   work branch for continued application testing.

## Investigation and candidate

The rebuilt original server reproduces SIGABRT; native backtrace ends in
`io_flush` → `server_advance` → `main`. `server_publish_owner` is selected as a
closed region despite queuing a newly allocated output buffer into
`state.reactor.connections` via `io_queue`. The queued buffer survives that
call and is later freed by `io_flush`.

The graph initialized boundary roots with empty memory. Field/raw loads from
those roots therefore dropped pointer provenance before a descendant Store#.
The small native `reclamation_incoming_storage.bv` reproduces the same failure:
Valgrind reports an invalid read and invalid free of a 32-byte payload released
inside `publish_nested` (exit 99), despite the program printing its success
marker. This is why output-only gates are insufficient.

Candidate: summarize externally supplied storage with a self-edge on the
analysis root's incoming parameters and global roots. Repeated field/raw loads
retain boundary provenance; local allocations keep precise field maps. Also
remove source entry-node names from closed callable selection after entry
mapping, preserving the documented subset invariant. Added graph, frontend,
native and Kani regressions. Verification pending.
