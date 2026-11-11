#!/usr/bin/env bash
# P6 (2026-11-11): interestingness predicates for compiler-bug reduction.
#
#   scripts/interesting.sh ice        <file.bv>   compiler crashes / panics
#   scripts/interesting.sh diagnostic <file.bv>   compile error that is NOT a crash
#   scripts/interesting.sh opt        <file.bv>   -O0 and -O3 disagree at runtime
#
# Exit 0 = interesting (reduce toward this), 1 = not interesting.
# Uses $BRIEVC when set, else target/debug/brievc.
set -u
pred=${1:?predicate required}
file=${2:?file required}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BRIEVC=${BRIEVC:-$ROOT/target/debug/brievc}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

run_build() {
    "$BRIEVC" build "$file" --llvm --out "$work/out" >"$work/build.log" 2>&1
}

is_ice() {
    grep -qiE 'panicked at|internal compiler error|RUST_BACKTRACE|[Ii]nternal error' "$work/build.log"
}

case "$pred" in
  ice)
    mkdir -p "$work/out"
    run_build
    status=$?
    # A panic exits 101 (or prints a panic marker even when wrapped).
    if [ "$status" -eq 101 ] || is_ice; then exit 0; fi
    exit 1
    ;;
  diagnostic)
    mkdir -p "$work/out"
    run_build
    status=$?
    if [ "$status" -ne 0 ] && ! is_ice; then exit 0; fi
    exit 1
    ;;
  opt)
    mkdir -p "$work/out"
    run_build || exit 1
    ll="$work/out/$(basename "${file%.bv}").ll"
    [ -f "$ll" ] || exit 1
    rt="$ROOT/lib/runtime/briev_rt.c"
    for level in 0 3; do
        clang "-O$level" -march=native "$ll" "$rt" -o "$work/bin$level" >"$work/link$level.log" 2>&1 || exit 1
    done
    for level in 0 3; do
        timeout 15 env BOUND=1000 "$work/bin$level" >"$work/out$level.txt" 2>&1
        echo $? >"$work/code$level"
    done
    if ! cmp -s "$work/out0.txt" "$work/out3.txt"; then exit 0; fi
    if ! cmp -s "$work/code0" "$work/code3"; then exit 0; fi
    exit 1
    ;;
  *)
    echo "unknown predicate '$pred' (ice|diagnostic|opt)" >&2
    exit 2
    ;;
esac
