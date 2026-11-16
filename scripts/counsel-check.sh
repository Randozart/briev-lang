#!/bin/bash
# Run counsel-check over src/, fail on Violated, print a clean census.
# Installs counsel-check from crates.io if not found.
set -euo pipefail

find_cc() {
    if command -v counsel-check &>/dev/null; then
        echo counsel-check
        return 0
    fi
    if command -v cargo &>/dev/null; then
        # Try to use counsel from this workspace, else fetch from crates.io
        if cargo metadata --no-deps -q /home/randozart/Desktop/Projects/counsel-crate/Cargo.toml &>/dev/null 2>&1; then
            echo "cargo run -q --release --manifest-path /home/randozart/Desktop/Projects/counsel-crate/Cargo.toml --features static --bin counsel-check --"
            return 0
        fi
        echo "cargo install counsel --quiet --no-default-features --features static 2>/dev/null && counsel-check"
        return 1
    fi
    return 1
}

CC_ARGS=()
# Resolve counsel-check binary
if ! CC=$(find_cc 2>/dev/null); then
    echo "Installing counsel-check from crates.io..." >&2
    cargo install counsel --quiet --no-default-features --features static 2>/dev/null || true
    CC=counsel-check
fi

echo "Running counsel-check over src/ ..."
WORKDIR=$(pwd)
JSON=$(mktemp)
trap 'rm -f "$JSON"' EXIT

$CC --format json src/ > "$JSON" 2>&1 && CC_RV=0 || CC_RV=$?

if [ $CC_RV -ne 0 ] && [ $CC_RV -ne 2 ]; then
    # Crash or parse error — surface it
    echo "counsel-check crashed (exit $CC_RV):" >&2
    cat "$JSON" >&2
    exit $CC_RV
fi

# Parse census from JSON
python3 - "$JSON" << 'EOF'
import json, sys
path = sys.argv[1]
try:
    with open(path) as f: data = json.load(f)
except Exception as e:
    print("error: failed to parse counsel-check JSON: {}".format(e), file=sys.stderr)
    sys.exit(2)

proven = unproven = skipped = violated = 0
for fi in data.get("files", []):
    for v in fi.get("verdicts", []):
        o = v.get("outcome", "unknown")
        if o == "proven":     proven += 1
        elif o == "unproven": unproven += 1
        elif o == "violated": violated += 1
        elif fi.get("skip_reason") or v.get("skip_reason"): skipped += 1

print("counsel-check census:")
print("  proven   : {}".format(proven))
print("  unproven : {}".format(unproven))
print("  skipped  : {}".format(skipped))
print("  violated : {}".format(violated))

if violated > 0:
    print("\nVIOLATED contracts found (false proofs):", file=sys.stderr)
    for fi in data.get("files", []):
        for v in fi.get("verdicts", []):
            if v.get("outcome") == "violated":
                print("  {} in {}".format(v["name"], fi.get("file","?")), file=sys.stderr)
    sys.exit(1)

if proven == 0 and unproven == 0 and skipped == 0:
    print("\nwarning: no contracts found — is counsel-check scanning the right scope?", file=sys.stderr)

sys.exit(0)
EOF
EOF
chmod +x scripts/counsel-check.sh && echo "script created, size=$(wc -c < scripts/counsel-check.sh) bytes"
