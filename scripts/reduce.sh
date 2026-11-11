#!/usr/bin/env bash
# P6 (2026-11-11): shrink a compiler-bug reproduction with cvise (preferred)
# or creduce, using scripts/interesting.sh as the predicate.
#
#   scripts/reduce.sh <ice|diagnostic|opt> <file.bv>
#
# Writes <file>.min.bv and leaves it for promotion into tests/compile_fail/
# or tests/directives/ with a regression pin.
set -euo pipefail
pred=${1:?predicate required (ice|diagnostic|opt)}
file=${2:?file required}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
interesting=$ROOT/scripts/interesting.sh
[ -x "$interesting" ] || { echo "missing $interesting" >&2; exit 2; }
[ -f "$file" ] || { echo "no such file: $file" >&2; exit 2; }

driver=$(mktemp)
trap 'rm -f "$driver"' EXIT
cat >"$driver" <<DRIVER
#!/bin/sh
exec "$interesting" "$pred" "\$1"
DRIVER
chmod +x "$driver"

if "$driver" "$file"; then
    echo "given file is already interesting ($pred); reducing" >&2
else
    echo "given file is NOT interesting ($pred); nothing to reduce" >&2
    exit 1
fi

tool=""
if command -v cvise >/dev/null 2>&1; then tool=cvise
elif command -v creduce >/dev/null 2>&1; then tool=creduce
else
    echo "neither cvise nor creduce is installed" >&2
    echo "install one (apt: cvise) or reduce by hand, then pin a fixture" >&2
    exit 3
fi

out="${file%.bv}.min.bv"
cp "$file" "$out"
echo "reducing with $tool -> $out" >&2
if [ "$tool" = cvise ]; then
    cvise "$driver" "$out"
else
    creduce "$driver" "$out"
fi
"$driver" "$out"
echo "reduced reproduction: $out"
