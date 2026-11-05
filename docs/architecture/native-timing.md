# Native timing boundary

2026-11-03: native load measurement exposed an inconsistent error path in
std/os/time.bv. The previous now_ns returned -1 despite its nonnegative
postcondition and loaded timespec fields before checking the syscall result.

clock_ns_res(clock_id) returns Result<Int,Int>: nonnegative nanoseconds or a
negative errno. Allocation failure is -12; negative or unrepresentable time
is -34. Output memory is read only after successful clock_gettime. This API
supports nonnegative clocks; it does not encode pre-epoch realtime values.

now_ns retains its existing nonnegative contract and CLOCK_MONOTONIC source.
It traps when time is unavailable; measurement clients needing recovery should
use clock_ns_res. A failed timestamp must never enter a latency histogram.

sleep_ms returns zero on completion and negative errno on failure, including
interruption. Callers that require deadline completion must reschedule using
monotonic time; a relative sleep alone does not promise completion on EINTR.

Regression coverage: tests/tier2_time.rs and
 tests/fixtures/load/clock_gate.bv exercise invalid clock IDs and elapsed sleep.
The boundary can be replaced when typed OS timespec support supplies the same
error, range and ownership guarantees.
