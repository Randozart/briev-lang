# Entropy output buffer lifetime

The Linux x86-64 getrandom syscall (318) was absent from the synchronous-borrow
effect table. A client using fresh WebSocket masking keys consequently suspended
allocation regions throughout its call chain, leaking otherwise temporary JSON
and strings. The kernel copies bytes before returning, including partial/error
results, and never retains the output pointer. Flags affect waiting/source,
not retention: https://man7.org/linux/man-pages/man2/getrandom.2.html.

Add the platform contract to config, without a Rust name/number dispatch arm.
Test dynamic flags and whole-call region inference; keep an unknown syscall as
the negative control. Verify the app's native memory gate, library suite and
changed-file Praetor baseline. Append the ownership architecture and bug ledger.
This corrects a missing effect contract, not an algorithm/performance change.
