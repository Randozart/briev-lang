# Bounded latency histograms

`std/stats/histogram.bv` supplies fixed-width, mergeable histograms for native
measurement programs. Construct one with `histogram_new(width, buckets)`;
width and samples use the same units. For nanosecond durations, a width of
1000000 gives one-millisecond bins. Storage is bounded by the chosen bucket
count, independently of the number of observations.

`histogram_record` records a nonnegative sample. Bin `i` covers
`[i * width, (i + 1) * width - 1]`. Samples outside the allocated bins increment
`overflow`; all samples increment `count` and contribute to `maximum`.
An exhausted sample counter traps instead of wrapping.

`histogram_quantile(h, permille)` uses the nearest rank, rounded upward.
For example, 500, 950 and 990 request p50, p95 and p99. The result is an
inclusive bin upper bound, not an exact observed percentile. When the rank
falls in overflow it returns the observed maximum, a conservative upper
bound. Empty distributions and permille values outside 1–1000 trap.
Reports must show width, bucket count, sample count and overflow count beside
percentiles. Never conceal overflow by presenting the last regular bin as p99.

`histogram_merge(left, right)` requires identical bin geometry and adds counts
and bins, retaining the larger maximum. Merge distributions before calculating
percentiles; averaging independently calculated percentiles is invalid.

Histograms are mutable objects: recording updates the supplied histogram and
merging updates the left histogram. Keep independently constructed objects for
independent workers, operations and outcomes. The returned handle supports
assignment-style use; it is not an immutable snapshot. Use the constructor and
these operations to preserve the relationship between bins, count and overflow.
The caller owns synchronization; the API does not promise atomic updates.

The native `tests/fixtures/load/histogram_gate.bv` gate checks a known uniform
distribution, the percentile boundary, overflow and merging. This is a
measurement-correctness gate, not a load or performance result.
