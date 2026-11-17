#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
"""Print one --verify-report document's counts as shell assignments.

2026-11-17 (skip-origin reporting): a report entry exists once per declaration
**per compilation unit**, and the prelude injects `std/iterator.bv` into every
native unit. Summing `len(d["skipped"])` across a corpus therefore counts 25
prelude declarations ~80 times, which made a corpus total read as far more
unmodelled program surface than exists.

This helper emits both numbers so a caller can report them honestly:

    eval "$(scripts/verify_report_counts.py report.json)"
    echo "$f_skipped entries, $f_skipped_own of them the file's own"

`*_own` comes from the report's `summary` block. A report written by a
pre-0.3.0 brievc has no `summary`; there the own count falls back to the bucket
total, which is the safe direction: it never understates a file's own surface.
"""

import json
import sys

BUCKETS = ("proven", "unproven", "skipped", "violated")


def counts(document):
    summary = document.get("summary") or {}
    values = {}
    for bucket in BUCKETS:
        total = len(document.get(bucket) or [])
        origin = summary.get(bucket) or {}
        values[bucket] = total
        values[f"{bucket}_own"] = origin.get("own", total)
    return values


def main(argv):
    if len(argv) != 2:
        print(f"usage: {argv[0]} <verify-report.json>", file=sys.stderr)
        return 2
    with open(argv[1], encoding="utf-8") as handle:
        values = counts(json.load(handle))
    for key, value in values.items():
        print(f"f_{key}={value}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
