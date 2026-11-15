#!/bin/bash
# Run counsel-check per-file over src/, fail on Violated, print a census.
#
# counsel-check is file-independent: it refuses a whole directory that contains
# even one unadmittable file (glob imports, unexpanded attribute macros, ...),
# so a whole-tree scan exits 2 and produces no census. Scanning each .rs file
# individually yields a real proven/unproven/violated/skipped census; refused
# files are counted as skipped, not as failures.
#
# Usage:
#   scripts/counsel-check.sh [counsel-check-binary]
#   (binary defaults to: counsel-check on PATH, else the sibling
#    ../counsel-crate/target/release/counsel-check build)
#
# Exit codes: 0 = no violated contracts, 1 = violated contract(s) found,
#             2 = counsel-check binary unavailable or scan error.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/src"

# Resolve the counsel-check binary.
CC="${1:-}"
if [ -z "$CC" ]; then
    if command -v counsel-check &>/dev/null; then
        CC=counsel-check
    elif [ -x "$ROOT/../counsel-crate/target/release/counsel-check" ]; then
        CC="$ROOT/../counsel-crate/target/release/counsel-check"
    else
        echo "counsel-check binary not found; pass it as the first argument" >&2
        exit 2
    fi
fi

echo "Running counsel-check per-file over src/ ..."
echo "binary: $CC"

TMP="$ROOT/scripts/.counsel-census.tmp"
: > "$TMP"
trap 'rm -f "$TMP"' EXIT

files_scanned=0
files_refused=0

# Iterate .rs files under src/ in deterministic (sorted) order.
# counsel-check exits non-zero (1) for scannable files whose strict gate fails
# (unannotated functions), and exits 2 (refusal) for files outside the subset.
# Both produce stdout we must distinguish: a scannable file emits a JSON
# document; a refused file emits a plain-text refusal note (no JSON).
# Capture stdout regardless of exit code, then test whether it is JSON.
while IFS= read -r f; do
    out=$("$CC" --format json "$f" 2>/dev/null) || true
    if [ -n "$out" ] && printf '%s' "$out" | head -c 1 | grep -q '^{' ; then
        files_scanned=$((files_scanned + 1))
        printf '%s\n' "$out" >> "$TMP"
    else
        files_refused=$((files_refused + 1))
    fi
done < <(find "$SRC" -name '*.rs' | sort)

# Aggregate the concatenated per-file JSON object streams into a census.
# Files-scanned/refused counts are passed as argv so the Python stage does not
# depend on shell state.
python3 - "$TMP" "$files_scanned" "$files_refused" << 'PYEOF'
import json, sys

path, scanned, refused = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
text = open(path).read()

proven = unproven = violated = skipped = 0
violated_detail = []
dec = json.JSONDecoder()
i, n = 0, len(text)
while i < n:
    while i < n and text[i] in ' \t\r\n':
        i += 1
    if i >= n:
        break
    try:
        obj, i = dec.raw_decode(text, i)
    except ValueError:
        break
    for fi in obj.get("files", []):
        for v in fi.get("verdicts", []):
            o = v.get("outcome", "unknown")
            if o == "proven":
                proven += 1
            elif o == "unproven":
                unproven += 1
            elif o == "violated":
                violated += 1
                violated_detail.append((v.get("name", "?"), fi.get("path", fi.get("file", "?"))))
            else:
                skipped += 1
        for s in fi.get("skips", []):
            skipped += 1

print("counsel-check census (per-file over src/):")
print("  files scanned : {}".format(scanned))
print("  files refused : {}".format(refused))
print("  proven        : {}".format(proven))
print("  unproven      : {}".format(unproven))
print("  skipped       : {}".format(skipped))
print("  violated      : {}".format(violated))

if violated > 0:
    print("\nVIOLATED contracts found (false proofs):", file=sys.stderr)
    for name, where in violated_detail:
        print("  {} in {}".format(name, where), file=sys.stderr)
    sys.exit(1)

sys.exit(0)
PYEOF
exit $?
