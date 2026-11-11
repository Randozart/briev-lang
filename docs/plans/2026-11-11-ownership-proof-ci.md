# Ownership proof CI investigation

Execution is CI-only at the user's request after local memory exhaustion.
Fork branch: `fix/closed-region-ownership`; no replacement PR is open.

- Run 43019562100: virtual-address limit rejected compiler allocation before
  checking. Replaced RLIMIT_AS with a 5 GiB resident-memory cgroup, zero swap,
  and 35-minute timeout. Exhaustion fails the job.
- Run 43020097219: reached CBMC; OOM while expanding registry hashing.
- Run 43021341305: regressions passed; proof OOM in OS random-seed setup even
  with empty registries.
- Run 43022711321: regressions passed; compositional harness compilation failed
  because NodeSet::contains is test-only. Switched to production iteration.
- Run 43023511951: regressions passed; proof OOM expanding B-tree internals at
  unwind 16. The next run uses unwind 4 for the fixed two-root, one-word fixture;
  unwinding assertions remain enabled. This is not a passing proof record.

The production solver now delegates storage facts to `storage.rs`; the proof
composes those same operations without constructing type/casting registries.
It checks publication through either field/raw access and another raw load.
Full solver scheduling and larger heap graphs are covered by ordinary unit and
native tests, not claimed as formally verified by this bounded harness.

Run 43024513676: regressions passed; unwind 4 still exhausted memory inside
B-tree node movement. The next harness instantiates the production storage
rules over an explicit one-word exact-set model sharing `merge_word` and
`WordNodes` with NodeSet. This removes B-tree internals from the proof scope;
it does not claim to have proved that library's implementation. Production
NodeSet remains covered by its ordered-set differential tests and full solver
regressions. Physical-memory and timeout limits remain enforced.
