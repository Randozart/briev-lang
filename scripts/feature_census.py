#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
"""Feature census — SPEC section -> fixture coverage (2026-11-19).

Every SPEC feature section should be exercised by at least one runnable
fixture (plan: docs/plans/2026-11-19-feature-example-coverage.md, F1).
Fixtures declare their section with a front-matter tag:

    // spec: 9.6          <- single section
    // spec: 11.2, 11.3   <- multiple sections (comma-separated)

The census walks spec/SPEC.md headings, collects tags from the fixture
roots (tests/tier1 by default; extra dirs appended), and reports:

  - COVERED     sections with >=1 declaring fixture
  - UNCOVERED   sections with zero fixtures (the actionable gap list)
  - BADTAG      fixtures whose tag does not resolve to a SPEC heading
                (typo or renumbered section — fix the tag)

Sections are identified by their numbered headings (`### 9.6 Cells`).
Unnumbered `##` headers (scope, staging) are out of scope: they are not
features. A section is also covered when one of its subsections is covered.

Usage: python3 scripts/feature_census.py [-v] [fixture-root ...]
       (default root: tests/tier1; -v lists the fixtures per covered section)
"""

import os
import re
import sys

DEFAULT_ROOTS = ["tests/tier1"]
SPEC = "spec/SPEC.md"
TAG_RE = re.compile(
    r"^\s*//\s*spec:\s*([0-9]+(?:\.[0-9]+)?(?:\s*,\s*[0-9]+(?:\.[0-9]+)?)*)\s*$"
)
HEAD_RE = re.compile(r"^(#{2,3})\s+([0-9]+(?:\.[0-9]+)*)\.?\s+")


def spec_sections():
    """Return {section_number: [children]} from SPEC.md numbered headings."""
    sections = {}
    for line in open(SPEC, errors="ignore"):
        m = HEAD_RE.match(line)
        if m:
            sections.setdefault(m.group(2), [])
    for sec in list(sections):
        parent = sec.rsplit(".", 1)[0] if "." in sec else None
        if parent in sections and parent != sec:
            sections[parent].append(sec)
    return sections


def collect_tags(roots, sections):
    """Return {section: [fixture_name]} and {fixture: bad_tag} for all roots."""
    covered = {}
    bad = {}
    for root in roots:
        register_root(root, sections, covered, bad)
    return covered, bad


def fixture_tags(path):
    """Return the `// spec:` tags from a fixture's leading comment block."""
    return [t for m in match_tag_lines(path) for t in m]


def match_tag_lines(path):
    """Yield stripped tag lists, one per matching `// spec:` line."""
    out = []
    with open(path, errors="ignore") as f:
        for line in f:
            if not line.startswith("//"):
                break
            m = TAG_RE.match(line)
            if m:
                out.append([t.strip() for t in m.group(1).split(",")])
    return out


def register_root(root, sections, covered, bad):
    """Record every tagged fixture of one root into covered/bad."""
    if not os.path.isdir(root):
        return
    for name in sorted(os.listdir(root)):
        if not name.endswith(".bv"):
            continue
        tags = fixture_tags(os.path.join(root, name))
        if not tags:
            continue
        fixture = name[:-3]
        for tag in tags:
            if tag in sections:
                covered.setdefault(tag, []).append(fixture)
            else:
                bad[fixture] = tag
    return covered, bad


def covered_ancestors(sec, covered):
    """True if any ancestor of `sec` (or `sec` itself) is directly covered."""
    while sec:
        if sec in covered:
            return True
        sec = sec.rsplit(".", 1)[0] if "." in sec else None
    return False


def main():
    args = [a for a in sys.argv[1:] if a != "-v"]
    verbose = "-v" in sys.argv
    roots = args or DEFAULT_ROOTS

    sections = spec_sections()
    covered, bad = collect_tags(roots, sections)

    uncovered = sorted(
        (s for s in sections if not covered_ancestors(s, covered)),
        key=lambda s: [int(p) for p in s.split(".")],
    )

    print(
        f"SPEC sections: {len(sections)}   covered: {len(sections) - len(uncovered)}"
        f"   uncovered: {len(uncovered)}"
    )
    if bad:
        print(f"\nBADTAG ({len(bad)}): tag does not resolve to a SPEC heading")
        for fixture, tag in sorted(bad.items()):
            print(f"  {fixture}: spec: {tag}")
    if uncovered:
        print("\nUNCOVERED sections:")
        for s in uncovered:
            print(f"  {s}")
    if verbose:
        print("\nCOVERED sections (fixtures per section):")
        for s in sorted(covered, key=lambda x: [int(p) for p in x.split(".")]):
            print(f"  {s}: {', '.join(covered[s])}")


if __name__ == "__main__":
    main()
