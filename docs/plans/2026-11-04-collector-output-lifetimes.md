# Collector output lifetime investigation

Use the optimized collector as its own process target for 20 samples at 100 Hz.
Compare allocation counts and CSV shape before and after each ownership change.
Retain the 5,000-sample, 500 Hz preflight as the sustained readiness gate.
This is a leak investigation. No throughput or latency claim is made.

On identical generated collector IR, the control lost 462 blocks directly and
61 indirectly. Releasing only the string produced and consumed inside print_int
after write_all reduced direct loss to 121 blocks, with the same 61 indirect
blocks. Both programs emitted the header and 20 records. This experimental
release is valid for those exact generated producers (malloc on both paths).

Implement integer output in the native Briev print family using the existing
buffered byte writer. Test signed extrema, both signs, zero and byte ordering,
and check the native program under Valgrind. Preserve the allocation behavior
of conversions that actually return a String. Document the print contract in
the stdlib rationale and the observed ownership failure in BUGS.md. Reprofile
the collector before attributing any remaining loss to file or enum ownership.

## Verified output change and remaining failures

The signed-extrema fixture prints exact bytes with one reachable 65,536-byte
stdout buffer and zero lost blocks or invalid accesses in Valgrind. The native
allocator-counter regression warms stdout, writes 2,048 min/max pairs across
a buffer flush, checks all 79,873 output bytes and checks the live allocation
count returns to its baseline. The old print lane fails that last condition.

All recommender load functional checks pass. At 20 samples and 100 Hz the
rebuilt collector loses 3,636 bytes in 121 blocks directly and 90,112 bytes in
60 blocks indirectly. At 5,000 samples and 500 Hz, RSS grows from 346 to 5,978
pages. The collector remains unsuitable for endurance measurements.

A separate probe calls read_file("/proc/self/stat"), checks its Result and
uses only the string length. One read loses a 16-byte result image directly
and its 4,104-byte string indirectly. Twenty reads lose exactly 320 direct
and 82,080 indirect bytes. The fixture has no collector, proc-stat parser or
numeric printing dependency. Any general ownership fix must distinguish
fresh storage from a borrowed/static payload of the same Result type.

### Last-use experiment

In the isolated successful-read probe, the generated Result image is a heap
handle. Unwrap yields the separate heap string. On that path the header length
load is the last use of both. Adding two releases immediately after the load
in the actual LLVM IR, then relinking at O3/LTO, removed all definite and
indirect losses at both one and twenty reads. Output matched the control and
Valgrind detected no invalid memory accesses. This experimental edit is not
part of the compiler or the collector.

A production lowering needs two independent facts: who owns the enum image,
and who owns the active payload. A caller-provided enum destination must not
be freed. A Result containing a static or borrowed string must not free it.
When either a result or its payload escapes (including an alias returned or
stored by another call), reclamation must follow that owner. A generic release
based only on Result<String,Int> would violate these rules. The app memory
gate now rebuilds the probe and collector and retains their Valgrind reports.

## Full collector diagnostic

At 20 successful samples the post-print control lost 3,576 bytes directly
in 121 blocks and 90,112 indirectly in 60 blocks. Releasing the pathname
temporaries, file result and payload, parser scratch storage, observations,
batch and startup auxv storage at their observed last uses in experimental
IR reduced both loss counts and Valgrind errors to zero. Across 5,000 samples
at 500 Hz, RSS went from 354 to 5,954 pages in the control and stayed at 366
pages at both endpoints in the experiment. Both emitted 5,000 valid rows.
These edits establish the successful path only. Production reclamation must
also preserve aliases, borrowed payloads, error returns and caller storage.
