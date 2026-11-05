# Linux process sampling

`std/os/proc_stat.bv` reads `/proc/PID/stat` and exposes the PID, start time,
user/system CPU ticks, minor/major page faults, thread count, virtual bytes
and resident pages. `proc_stat_parse` is independently testable. Both public
functions return `Result`; malformed required fields return `Err(-22)`,
integer overflow returns `Err(-34)`, and file errors preserve negative errno.

Field positions follow the [Linux kernel proc documentation](https://docs.kernel.org/filesystems/proc.html).
The parser isolates the parenthesized command using its final closing
parenthesis before scanning fields. Process names containing spaces and
parentheses therefore do not shift CPU or memory columns. It accepts additional
fields after RSS for forward compatibility and ignores unneeded field values.

Counters retain their original units. Consumers must record the platform's
clock-tick frequency and page size; do not assume ticks are milliseconds or
pages are 4096 bytes. Compute CPU deltas only between samples with equal PID
and start time. A counter decrease or changed identity invalidates the delta.
Sample the monotonic clock around the read to expose collection uncertainty.

RSS is an approximate resident-page count, not unique or private memory.
Summing it across backend workers double-counts shared pages. More expensive
`smaps`/PSS sampling, descriptor counts, I/O and child discovery are separate
collector capabilities still to implement. A process can exit during a read;
report a missing sample rather than replacing it with zeros.

The native gate checks synthetic field positions, names with parentheses,
truncation, decimal overflow and a real sample of its own running process.
These are parser and OS-integration checks, not measurements of app capacity.

`std/os/auxv.bv` supplies `process_units()` on Linux x86_64. It reads
`/proc/self/auxv` and returns `ProcessUnits { page_bytes, ticks_per_second }`
from AT_PAGESZ and AT_CLKTCK. These are the same units queried through
sysconf; see the [Linux auxiliary-vector documentation](https://man7.org/linux/man-pages/man3/getauxval.3.html).
The pure `auxv_value` parser requires complete 16-byte entries and an AT_NULL
terminator, rejects duplicate requested tags and values outside nonnegative
Int range, and distinguishes malformed input from a missing tag. File errors
are preserved. This layout targets the existing Linux x86_64 OS family.

The native parser gate uses synthetic page/tick values that differ from this
machine's defaults. An optimized integration gate independently compares live
values against `getconf PAGESIZE` and `getconf CLK_TCK`; positivity alone does
not establish correctness of unit conversion.
