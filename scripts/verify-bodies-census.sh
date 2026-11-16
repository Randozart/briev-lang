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
ENTRY_COUNT=0

# Write JSON header
cat > "$JSON_OUT" <<'HEADER'
{
  "total_files": PLACEHOLDER,
  "summary": {
    "proven": 0,
    "unproven": 0,
    "skipped": 0,
    "violated": 0,
    "timed_out": 0
  },
  "files": [
HEADER

while IFS= read -r line || [[ -n "$line" ]]; do
  # Skip blank lines and comments
  [[ -z "$line" || "$line" =~ ^# || "$line" =~ ^--- ]] && continue

  TOTAL=$((TOTAL + 1))
  f="$line"

  # Run brievc --verify-report (writes JSON to TMP_JSON).
  # --no-link skips the link step: it produces an identical report (verified on
  # .smoke and lib/std files) and halves the wall time, which is what lets this
  # census fit in CI.
  f_timed_out=0
  if [[ "$PER_FILE_TIMEOUT" == "0" ]]; then
    "$BRIEVC" build "$f" --verify-report "$TMP_JSON" --no-link 2>/dev/null || true
  else
    timeout --kill-after=10 "$PER_FILE_TIMEOUT" \
      "$BRIEVC" build "$f" --verify-report "$TMP_JSON" --no-link 2>/dev/null \
      || { [[ $? -eq 124 ]] && f_timed_out=1; }
  fi

  if [[ -f "$TMP_JSON" ]]; then
    f_proven=$(python3 -c "import json; d=json.load(open('$TMP_JSON')); print(len(d.get('proven',[])))" 2>/dev/null || echo 0)
    f_unproven=$(python3 -c "import json; d=json.load(open('$TMP_JSON')); print(len(d.get('unproven',[])))" 2>/dev/null || echo 0)
    f_skipped=$(python3 -c "import json; d=json.load(open('$TMP_JSON')); print(len(d.get('skipped',[])))" 2>/dev/null || echo 0)
    f_violated=$(python3 -c "import json; d=json.load(open('$TMP_JSON')); print(len(d.get('violated',[])))" 2>/dev/null || echo 0)
    rm -f "$TMP_JSON"
  else
    f_proven=0
    f_unproven=0
    f_skipped=0
    f_violated=0
  fi

  PROVEN=$((PROVEN + f_proven))
  UNPROVEN=$((UNPROVEN + f_unproven))
  SKIPPED=$((SKIPPED + f_skipped))
  VIOLATED=$((VIOLATED + f_violated))
  TIMED_OUT=$((TIMED_OUT + f_timed_out))

  # Comma separator between entries
  if [[ $ENTRY_COUNT -gt 0 ]]; then
    echo "," >> "$JSON_OUT"
  fi
  ENTRY_COUNT=$((ENTRY_COUNT + 1))

  # Write per-file entry
  printf '    {"file": "%s", "proven": %d, "unproven": %d, "skipped": %d, "violated": %d, "timed_out": %d}' \
    "$f" "$f_proven" "$f_unproven" "$f_skipped" "$f_violated" "$f_timed_out" >> "$JSON_OUT"

  # Progress every 100 files
  if (( TOTAL % 100 == 0 )); then
    echo "  $TOTAL files, proven=$PROVEN unproven=$UNPROVEN violated=$VIOLATED" >&2
  fi
done < "$CORPUS"

# Close JSON
cat >> "$JSON_OUT" <<FOOTER
  ]
}
FOOTER

# Replace placeholder with actual total and populate summary counters
sed -i "s/PLACEHOLDER/$TOTAL/" "$JSON_OUT"
sed -i "s/\"proven\": 0/\"proven\": $PROVEN/" "$JSON_OUT"
sed -i "s/\"unproven\": 0/\"unproven\": $UNPROVEN/" "$JSON_OUT"
sed -i "s/\"skipped\": 0/\"skipped\": $SKIPPED/" "$JSON_OUT"
sed -i "s/\"violated\": 0 /\"violated\": $VIOLATED,/" "$JSON_OUT" 2>/dev/null || true
sed -i "s/\"violated\": 0,\$/\"violated\": $VIOLATED,/" "$JSON_OUT" 2>/dev/null || true
sed -i "s/\"timed_out\": 0/\"timed_out\": $TIMED_OUT/" "$JSON_OUT"

echo "Summary: total=$TOTAL proven=$PROVEN unproven=$UNPROVEN skipped=$SKIPPED violated=$VIOLATED timed_out=$TIMED_OUT" >&2
if [[ $TIMED_OUT -gt 0 ]]; then
  echo "WARNING: $TIMED_OUT file(s) hit the ${PER_FILE_TIMEOUT}s per-file bound; they are recorded as timed_out, not counted as proven." >&2
fi

# Violated contracts are errors (false proofs)
if [[ $VIOLATED -gt 0 ]]; then
  echo "ERROR: VIOLATED contracts found (false proofs!):" >&2
  exit 1
fi

exit 0
