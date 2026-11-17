#!/usr/bin/env bash
# verify-bodies-census.sh — Run --verify-bodies --verify-report on a corpus
# and produce a single JSON artifact suitable for CI archiving.
#
# Usage: scripts/verify-bodies-census.sh <file-list> [output.json]
#   Files in the list must be paths relative to the worktree root.
#   Blank lines and lines starting with # are skipped.
#
# Output: JSON to the output file.
#         Summary line to stderr.
# Exit 0 unless a VIOLATED contract is found (false proof).
set -euo pipefail

# Resolve the helper beside this script, so the census works from any cwd (CI
# runs it from the repo root; a developer may not).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CORPUS="${1:?Usage: verify-bodies-census.sh <file-list> [output.json]}"
JSON_OUT="${2:-/tmp/verify-bodies-report.json}"
BRIEVC="${BRIEVC:-/home/randozart/.cache/counsel/brievc-v0.1.4-brievc-linux-x86_64/bin/brievc}"
TMP_JSON="/tmp/vr-tmp.json"
# 2026-11-16: per-file wall-clock bound. The CI job used to die after ~55 min
# with "the hosted runner lost communication" — one pathological file (or the
# accumulated cost of a few) starved the runner instead of being recorded. Bounded
# here so a slow file becomes a reported `timed_out` entry instead of a dead job.
# Override with VERIFY_BODIES_TIMEOUT (seconds) to widen or disable (0 = off).
PER_FILE_TIMEOUT="${VERIFY_BODIES_TIMEOUT:-120}"
TIMED_OUT=0

TOTAL=0
PROVEN=0
UNPROVEN=0
SKIPPED=0
VIOLATED=0
# Own-surface tallies (2026-11-17): the declarations belonging to the file being
# compiled, excluding everything its imports and the prelude contributed.
PROVEN_OWN=0
UNPROVEN_OWN=0
SKIPPED_OWN=0
ERRORS=0
ENTRY_COUNT=0

# Per-file entries are buffered and the document is assembled once at the end.
#
# Do NOT build the header first and patch it with `sed -i "s/\"proven\": 0/.../"`:
# sed without an address rewrites EVERY match in the document, so each file's
# `"unproven": 0` became the grand total — the full corpus looked like
# "5 unproven in all 92 files" while every file individually reported 0.
ENTRIES="$(mktemp)"
trap 'rm -f "$ENTRIES"' EXIT

while IFS= read -r line || [[ -n "$line" ]]; do
  # Skip blank lines and comments
  [[ -z "$line" || "$line" =~ ^# || "$line" =~ ^--- ]] && continue

  TOTAL=$((TOTAL + 1))
  f="$line"

  # Run brievc --verify-report (writes JSON to TMP_JSON).
  # --no-link skips the link step: it produces an identical report (verified on
  # .smoke and lib/std files) and halves the wall time, which is what lets this
  # census fit in CI.
  #
  # The temp file is removed BEFORE the run, never after. A file that fails to
  # build writes no report, so leaving the previous file's JSON in place made its
  # counts reappear against this file — the full-corpus run once reported
  # "5 unproven" on every single file while the authoritative runner found
  # exactly 5 unproven obligations in total.
  rm -f "$TMP_JSON"
  f_error=0
  f_timed_out=0
  if [[ "$PER_FILE_TIMEOUT" == "0" ]]; then
    "$BRIEVC" build "$f" --verify-report "$TMP_JSON" --no-link 2>/dev/null || f_error=1
  else
    timeout --kill-after=10 "$PER_FILE_TIMEOUT" \
      "$BRIEVC" build "$f" --verify-report "$TMP_JSON" --no-link 2>/dev/null \
      || { rc=$?; [[ $rc -eq 124 || $rc -eq 137 ]] && f_timed_out=1 || f_error=1; }
  fi

  if [[ $f_error -eq 0 && -f "$TMP_JSON" ]] && COUNT_ASSIGN="$(python3 "$SCRIPT_DIR/verify_report_counts.py" "$TMP_JSON")"; then
    # One helper, both tallies: `*_own` counts the declarations that belong to
    # the file being compiled. The raw totals repeat every prelude declaration
    # in every unit (std/iterator.bv: 25 generics x ~80 files), so both are
    # reported and neither is silently substituted for the other.
    eval "$COUNT_ASSIGN"
  else
    f_proven=0
    f_unproven=0
    f_skipped=0
    f_violated=0
    f_proven_own=0
    f_unproven_own=0
    f_skipped_own=0
  fi
  # A timeout can land *after* brievc has written the report (verification is
  # earlier in the pipeline than codegen), in which case the verdicts are real
  # and the cut only cost us the build. `timed_out` therefore means exactly
  # "verification did not complete", never "the build was slow".
  if [[ $f_timed_out -eq 1 && -f "$TMP_JSON" ]]; then
    f_timed_out=0
  fi
  rm -f "$TMP_JSON"

  PROVEN=$((PROVEN + f_proven))
  UNPROVEN=$((UNPROVEN + f_unproven))
  SKIPPED=$((SKIPPED + f_skipped))
  VIOLATED=$((VIOLATED + f_violated))
  PROVEN_OWN=$((PROVEN_OWN + f_proven_own))
  UNPROVEN_OWN=$((UNPROVEN_OWN + f_unproven_own))
  SKIPPED_OWN=$((SKIPPED_OWN + f_skipped_own))
  ERRORS=$((ERRORS + f_error))
  TIMED_OUT=$((TIMED_OUT + f_timed_out))

  # Comma separator between entries
  if [[ $ENTRY_COUNT -gt 0 ]]; then
    echo "," >> "$ENTRIES"
  fi
  ENTRY_COUNT=$((ENTRY_COUNT + 1))

  # Write per-file entry
  printf '    {"file": "%s", "proven": %d, "unproven": %d, "skipped": %d, "violated": %d, "proven_own": %d, "unproven_own": %d, "skipped_own": %d, "error": %d, "timed_out": %d}' \
    "$f" "$f_proven" "$f_unproven" "$f_skipped" "$f_violated" \
    "$f_proven_own" "$f_unproven_own" "$f_skipped_own" "$f_error" "$f_timed_out" >> "$ENTRIES"

  # Progress every 100 files
  if (( TOTAL % 100 == 0 )); then
    echo "  $TOTAL files, proven=$PROVEN unproven=$UNPROVEN violated=$VIOLATED" >&2
  fi
done < "$CORPUS"

# Assemble the document once: summary from the computed totals, then the
# buffered per-file entries. No post-hoc `sed` over the whole file.
{
  printf '{\n  "total_files": %d,\n' "$TOTAL"
  # Raw counts stay where they were: a consumer that already reads
  # `summary.skipped` keeps working. `own` sits beside them and is the number to
  # quote for unmodelled PROGRAM surface, because a raw count repeats every
  # prelude declaration once per compilation unit.
  printf '  "summary": {\n    "proven": %d,\n    "unproven": %d,\n    "skipped": %d,\n    "violated": %d,\n    "errors": %d,\n    "timed_out": %d,\n    "own": {\n      "proven": %d,\n      "unproven": %d,\n      "skipped": %d\n    }\n  },\n' \
    "$PROVEN" "$UNPROVEN" "$SKIPPED" "$VIOLATED" "$ERRORS" "$TIMED_OUT" \
    "$PROVEN_OWN" "$UNPROVEN_OWN" "$SKIPPED_OWN"
  printf '  "files": [\n'
  cat "$ENTRIES"
  printf '\n  ]\n}\n'
} > "$JSON_OUT"

echo "Summary: total=$TOTAL proven=$PROVEN unproven=$UNPROVEN skipped=$SKIPPED violated=$VIOLATED errors=$ERRORS timed_out=$TIMED_OUT" >&2
# Own-surface line (2026-11-17): what belongs to the corpus files themselves.
# Quote this one for unmodelled program surface; the raw line above counts every
# prelude declaration once per compilation unit (`std/iterator.bv` alone adds 25
# generics to each native file).
echo "Summary (own): proven=$PROVEN_OWN unproven=$UNPROVEN_OWN skipped=$SKIPPED_OWN" >&2
if [[ $TIMED_OUT -gt 0 ]]; then
  echo "WARNING: $TIMED_OUT file(s) had no verdict: the ${PER_FILE_TIMEOUT}s per-file bound cut verification short." >&2
fi

# Violated contracts are errors (false proofs)
if [[ $VIOLATED -gt 0 ]]; then
  echo "ERROR: VIOLATED contracts found (false proofs!):" >&2
  exit 1
fi

exit 0
