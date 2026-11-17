#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
#
# Corpus body-verification measurement.
#
# 2026-11-15: the z3-escalation baseline was produced by an ad-hoc shell loop,
# which made before/after comparisons unreliable (the report was accidentally
# appended three times, tripling every per-goal count). This script is the one
# reproducible runner: single pass, sorted file order, explicit summary, and a
# per-goal tally so a change in one class cannot hide behind aggregate noise.
#
# Usage:
#   bash scripts/verify_bodies_corpus.sh                 # whole corpus
#   bash scripts/verify_bodies_corpus.sh lib/std         # one directory
#   bash scripts/verify_bodies_corpus.sh --full          # also print every warning
#   BRIEVC=./target/release/brievc bash scripts/verify_bodies_corpus.sh
#
# 2026-11-15: builds are redirected with --out into the scratch directory. Without
# it the runner dropped an ELF executable next to every lib/std source file (85
# untracked artifacts in one run) — a measurement tool must not write into the
# tree it is measuring.
#
# Z3 escalation makes the whole corpus a ~20 min job, so targeted runs are the
# normal case while iterating on a single class. Files are verified in parallel
# (one process per file, output reassembled in sorted order).
#
# Output buckets:
#   PROVEN             body postcondition discharged
#   UNPROVEN           obligation the prover could not decide
#   NO_POSTCONDITION   body has no `term`/`endprogram` — nothing to verify.
#                      Correct behaviour, counted separately so it never
#                      dilutes the unproven denominator.
#   SKIPPED            body outside the analysis surface (no value summary,
#                      type outside the word model, ...)
#   VIOLATED / ERR     real failures
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

BRIEVC="${BRIEVC:-./target/debug/brievc}"
FULL=0
ARGS=()
for arg in "$@"; do
    case "$arg" in
    --full) FULL=1 ;;
    *) ARGS+=("$arg") ;;
    esac
done

if [ "${#ARGS[@]}" -gt 0 ]; then
    CORPORA=("${ARGS[@]}")
else
    CORPORA=(tests/tier1 tests/tier2 trophies lib/std examples)
fi

# 2026-11-15 (corpus hygiene): host-corpus exclusions. The runner builds every
# file under the HOST target; files that are not host-buildable must be listed
# here with a reason. Every line must be one path (sorted) — the grep -vF -f
# below drops exactly those lines from the file list.
HOST_EXCLUDES="$(mktemp)"
cat >"$HOST_EXCLUDES" <<'EOF'
# target-specific: `target:` directive / `.b` embedded dialect, no x86_64 path
examples/bad/boot_mps2.bv
examples/bad/boot_rv64.bv
examples/bad/boot_stage1.bv
# 2026-11-17 (corpus triage): the `.bad` dialect is the BARE-METAL dialect
# (docs/architecture/bad-dialect.md), not "invalid source". These are valid
# multi-target bootloaders — `bootloader.bv` boots riscv64 and `typed_boot.bv`
# is its typed end-to-end proof — and they have no host build path. Leaving them
# in the corpus charged the floor for a Verifier verdict on a program that is
# not host-buildable, the same reason the three above are excluded.
examples/bad/bootloader.bv
examples/bad/typed_boot.bv
examples/kernel_rv64.b.bv
examples/timer_arm.b.bv
examples/timer_rv64.b.bv
# missing external dependency at the build host (external library or source)
examples/glue-macro.bv
lib/std/ai/openvino.bv
lib/std/image/stb_image.bv
# browser-alternative prelude: the native .bv prelude already injects cast_lanes,
# so checking cast_portable under it double-injects the shared defns. Exercised
# by every .rbv build instead (see src/conformance.rs).
lib/std/cast_portable.bv
# blocked by the string value-summary lane bug + string callee liveness gap
# (BUGS.md 2026-11-15 corpus hygiene); full string behavior is covered at
# runtime by tests/tier1/unicode_search_smoke.bv.
tests/tier2/test_string_ops.bv
EOF
sed -i -e '/^#/d' -e '/^$/d' "$HOST_EXCLUDES"

mapfile -t FILES < <(
    for dir in "${CORPORA[@]}"; do
        [ -d "$dir" ] || continue
        find "$dir" -type f \( -name '*.bv' -o -name '*.rbv' \) | LC_ALL=C sort
    done | grep -vF -f "$HOST_EXCLUDES"
)

# 2026-11-15 (corpus hygiene): trophies/ is a showcase/parity directory, not a
# live test surface. The #!exit removal (bddf015ea) swept the `#!exit` lines
# but the files are still stale for *other* removed surfaces (`#out` frgn form,
# pre-width-suffix literal tokens, ...). Migrating them through every removed
# surface is its own workstream; the corpus gate measures live host-buildable
# code, not stale showcase. Excluded here; individual files can be re-included
# as they are migrated.
if [[ " ${CORPORA[*]} " == *" trophies "* ]]; then
    _filtered=()
    for f in "${FILES[@]}"; do
        [[ "$f" == trophies/* ]] || _filtered+=("$f")
    done
    FILES=("${_filtered[@]}")
fi

if [ "${#FILES[@]}" -eq 0 ]; then
    echo "no corpus files found" >&2
    exit 1
fi

raw="$(mktemp)"
trap 'rm -f "$raw"' EXIT

# One compiler process per file, N at a time; results land in per-file temp
# files so the final tally is deterministic regardless of scheduling.
workdir="$(mktemp -d)"
trap 'rm -f "$raw"; rm -rf "$workdir"' EXIT

printf '%s\0' "${FILES[@]}" \
    | xargs -0 -P "$(nproc)" -I{} sh -c '
        f="$1"
        key="$(printf "%s" "$f" | tr "/" "_")"
        out="$2/$key"
        # 2026-11-17 (corpus triage): every file needs its OWN --out dir. The
        # runner used to share "$3/out", so parallel builds overwrote each
        # other"s `<name>.ll` and `opt` then verified one file against another
        # — `examples/wildcard_lift.bv` came back carrying a dozen unrelated
        # verifier errors while the files that actually owned them looked
        # clean. A measurement tool that cross-attributes failures cannot
        # produce a trustworthy floor.
        mkdir -p "$2/build-$key"
        { printf "FILE: %s\n" "$f"
          "$0" build "$f" --verify-bodies --out "$2/build-$key" 2>&1 \
              | grep -E "could not prove|verify-bodies skipped|verify-bodies violation|error:"
        } > "$out" 2>&1
    ' "$BRIEVC" {} "$workdir" "$workdir"

for file in "${FILES[@]}"; do
    cat "$workdir/$(printf '%s' "$file" | tr '/' '_')" 2>/dev/null >>"$raw"
done

[ "$FULL" -eq 1 ] && cat "$raw"

# Tally. Done in awk: one pass, per-file PROVEN (a file with no failure
# bucket), and NO_POSTCONDITION counted separately so it never inflates the
# unproven denominator.
counts=$(awk '
    /^FILE: / { if (seen && !bad) proven++; seen=1; bad=0; next }
    /body has no terminating value path/ { no_post++; next }
    /could not prove:/ { unproven++; bad=1; next }
    /verify-bodies skipped:/ { skipped++; next }
    /verify-bodies violation:/ { violated++; bad=1; violations = violations $0 "\n"; next }
    /error:/ { errors++; bad=1; errlines = errlines $0 "\n"; next }
    END {
        if (seen && !bad) proven++
        printf "%d %d %d %d %d %d", proven, unproven, no_post, skipped, violated, errors
    }
' "$raw")

read -r proven unproven no_post skipped violated errors <<<"$counts"
violated_lines=$(awk '/verify-bodies violation:/' "$raw")
error_lines=$(awk '/error:/' "$raw")

cat <<EOF
=== VERIFY-BODIES CORPUS ===
Files:            ${#FILES[@]}
PROVEN:           $proven
UNPROVEN:         $unproven
NO_POSTCONDITION: $no_post
SKIPPED:          $skipped
VIOLATED:         $violated
ERR:              $errors
EOF

if [ -n "$violated_lines" ]; then
    printf '\n=== VIOLATIONS ===\n%s' "$violated_lines"
fi

if [ -n "$error_lines" ]; then
    printf '\n=== FILE ERRORS (not proof failures) ===\n%s' "$error_lines"
fi

echo
echo "=== unproven goals by function|goal ==="
grep 'could not prove:' "$raw" \
    | grep -v 'body has no terminating value path' \
    | sed -E 's/.*could not prove: ([A-Za-z_0-9$]+): .*goal: (.*)\) — add a guard.*/\1 | \2/; s/.*could not prove: ([A-Za-z_0-9$]+): (.*)/\1 | \2/' \
    | LC_ALL=C sort | uniq -c | LC_ALL=C sort -rn
