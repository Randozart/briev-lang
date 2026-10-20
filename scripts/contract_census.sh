#!/usr/bin/env bash
# Contract-gate census (diagnostic only). Runs the conformance sweep under
# `policy: warn` through a temporary BRIEV_HOME overlay, so the tracked
# `config/contracts.dbvl` is never edited. Emits the raw log and a per-callee
# announcement count. See docs/plans/2026-10-19-contract-tiers-and-regularity.md.
set -euo pipefail
cd "$(dirname "$0")/.."

out="${1:-/tmp/opencode/census_$(date +%Y%m%d_%H%M%S).log}"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/briev-contract-census.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

ln -s "$PWD/lib" "$tmp/lib"
cp -R "$PWD/config" "$tmp/config"
printf 'policy: warn;\n' > "$tmp/config/contracts.dbvl"

# Prebuild outside the overlay so the compiler's resource-root probe cannot
# influence cargo's fingerprinting. Then run the same binary under BRIEV_HOME.
cargo test --lib conformance_sweep --no-run >/dev/null
test_bin="$(cargo test --lib conformance_sweep --no-run --message-format=json 2>/dev/null \
    | python3 -c 'import json,sys
for line in sys.stdin:
    try: m=json.loads(line)
    except Exception: continue
    if m.get("reason")=="compiler-artifact" and m.get("executable"):
        print(m["executable"]); break')"
if [ -z "$test_bin" ]; then
    echo "contract census: could not locate conformance test binary" >&2
    exit 1
fi

mkdir -p "$(dirname "$out")"
BRIEV_HOME="$tmp" "$test_bin" 'conformance::tests::conformance_sweep_every_active_source_parses_and_checks' --exact --nocapture >"$out" 2>&1

total="$(sed -n "s/^warning: call to '\([^']*\)'.*/\1/p" "$out" | wc -l)"
unique="$(sed -n "s/^warning: call to '\([^']*\)'.*/\1/p" "$out" | sort -u | wc -l)"
{
    echo "contract census: $total announcements / $unique contracted fns"
    echo "log: $out"
    sed -n "s/^warning: call to '\([^']*\)'.*/\1/p" "$out" \
        | sort | uniq -c | sort -rn
} | tee "${out%.log}.summary.txt"
