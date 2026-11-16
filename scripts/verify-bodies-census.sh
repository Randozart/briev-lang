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
    "violated": 0
  },
  "files": [
HEADER

while IFS= read -r line || [[ -n "$line" ]]; do
  # Skip blank lines and comments
  [[ -z "$line" || "$line" =~ ^# || "$line" =~ ^--- ]] && continue

  TOTAL=$((TOTAL + 1))
  f="$line"

  # Run brievc --verify-report (writes JSON to TMP_JSON)
  "$BRIEVC" build "$f" --verify-report "$TMP_JSON" 2>/dev/null || true

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

  # Comma separator between entries
  if [[ $ENTRY_COUNT -gt 0 ]]; then
    echo "," >> "$JSON_OUT"
  fi
  ENTRY_COUNT=$((ENTRY_COUNT + 1))

  # Write per-file entry
  printf '    {"file": "%s", "proven": %d, "unproven": %d, "skipped": %d, "violated": %d}' \
    "$f" "$f_proven" "$f_unproven" "$f_skipped" "$f_violated" >> "$JSON_OUT"

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
sed -i "s/\"violated\": 0/\"violated\": $VIOLATED/" "$JSON_OUT"

echo "Summary: total=$TOTAL proven=$PROVEN unproven=$UNPROVEN skipped=$SKIPPED violated=$VIOLATED" >&2

# Violated contracts are errors (false proofs)
if [[ $VIOLATED -gt 0 ]]; then
  echo "ERROR: VIOLATED contracts found (false proofs!):" >&2
  exit 1
fi

exit 0
