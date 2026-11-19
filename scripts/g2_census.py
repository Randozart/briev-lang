#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
"""G2 census — `term;` checkpoints inside `when` guards (2026-11-19).

A `when` guard whose body ENDS in bare `term;` is a convergence checkpoint:
execution continues to the statement after the guard, it does not exit. The
compiler warns about this shape (G2, dogfooding-gap ledger). Before promoting
that warning to a hard error, every existing site must be classified and —
where the author meant an exit — rewritten to the exit spelling (`term! ->`).

Output: per-site CHECKPOINT (statements follow the guard — the ambiguous class
the promotion would break) and TRAILING (the guard is block-final, where
`term;` and `term! ->` are observationally equivalent and the rewrite is a
no-op).

Usage: python3 scripts/g2_census.py [-v] [paths ...]
       (default paths: lib examples tests/tier1; -v lists every site)
"""

import glob
import sys

DEFAULT_PATHS = ["lib", "examples", "tests/tier1"]


def classify(path: str):
    """Yield (line_no_1based, kind, next_statement) for guard-final `term;`."""
    try:
        lines = open(path, errors="ignore").read().split("\n")
    except OSError:
        return
    for i, line in enumerate(lines):
        stripped = line.strip()
        if not (stripped.startswith("when ") and stripped.endswith("{")):
            continue
        depth = 1
        j = i
        while depth > 0 and j + 1 < len(lines):
            j += 1
            depth += lines[j].count("{") - lines[j].count("}")
        body = [
            b.strip()
            for b in lines[i + 1 : j]
            if b.strip() and not b.strip().startswith("//")
        ]
        if not body or body[-1] != "term;":
            continue
        after = [
            a.strip()
            for a in lines[j + 1 :]
            if a.strip() and not a.strip().startswith("//")
        ]
        if after:
            yield i + 1, "CHECKPOINT", after[0][:60]
        else:
            yield i + 1, "TRAILING", ""


def main() -> None:
    argv = sys.argv[1:]
    verbose = "-v" in argv
    paths = [a for a in argv if not a.startswith("-")] or DEFAULT_PATHS
    files = sorted(
        set(f for p in paths for f in glob.glob(p + "/**/*.bv", recursive=True))
    )
    checkpoints = []
    trailing = 0
    for path in files:
        if not path.endswith(".bv"):
            continue
        for line_no, kind, nxt in classify(path):
            if kind == "CHECKPOINT":
                checkpoints.append(f"  {path}:{line_no} -> next: {nxt}")
            else:
                trailing += 1
    print(f"G2 census: {len(checkpoints)} checkpoint(s), {trailing} trailing")
    if verbose:
        for site in checkpoints:
            print(site)


if __name__ == "__main__":
    main()
