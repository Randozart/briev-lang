#!/usr/bin/env bash
# Effect-shape regression guard.
#
# Structural guard (default, CI-safe):
#   - compiles representative benchmarks and asserts which effect-memory
#     shape the target-derived selector chose;
#   - compiles the mixed program twice (auto + forced stateless) and compares
#     sorted output, proving both shapes remain correct.
#
# Timing guard (opt-in, local):
#   bash benchmarks/effect_shape_guard.sh --timing
#   Runs the harness for the primary cases and enforces wide catastrophic
#   regression ceilings, not noisy parity thresholds.
#
# The expected decisions are the regression pin for
# docs/architecture/effect-shape-selection.md. If a case flips, inspect the
# loop shape facts first (`BRIEV_DIAG_LOOP=1`), not the benchmark timing.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BRIEVC="${BRIEVC:-$ROOT/target/release/brievc}"
MODE="${1:-}"
case "$MODE" in
  ""|--quick) TIMING=0 ;;
  --timing)   TIMING=1 ;;
  *) echo "usage: $0 [--quick|--timing]" >&2; exit 2 ;;
esac

if [[ ! -x "$BRIEVC" ]]; then
  echo "error: compiler not found at $BRIEVC (cargo build --release --bin brievc)" >&2
  exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
FAIL=0

shape_of() {
  if grep -Fq "info: effect shape escaped" "$1"; then
    echo escaped
  elif grep -Fq "info: effect state carried" "$1"; then
    echo carry
  else
    echo stateless
  fi
}

check_shape() {
  local name="$1" path="$2" expect="$3"
  local out="$TMP/shape_$name"
  mkdir -p "$out"
  local log="$out/build.log"
  if ! "$BRIEVC" build "$path" --out "$out" >"$log" 2>&1; then
    echo "FAIL $name: build failed (see $log)" >&2
    tail -20 "$log" >&2
    FAIL=1
    return
  fi
  local got
  got="$(shape_of "$log")"
  if [[ "$got" != "$expect" ]]; then
    echo "FAIL $name: expected $expect effect shape, got $got" >&2
    grep -F "effect shape" "$log" >&2 || true
    FAIL=1
    return
  fi
  echo "ok   $name: $got"
}

check_shape float_math benchmarks/float_math.bv escaped
check_shape float_math_nonzero benchmarks/float_math_nonzero.bv carry
check_shape kalman_filter_runtime benchmarks/kalman_filter_runtime.bv carry
check_shape telemetry_stream benchmarks/telemetry_stream.bv escaped
check_shape pid_control benchmarks/pid_control.bv escaped
check_shape accumulator_flush benchmarks/accumulator_flush.bv escaped
check_shape sweep_sparse benchmarks/sweep_sparse.bv carry
check_shape sweep_mid benchmarks/sweep_mid.bv escaped
check_shape queue_drain benchmarks/queue_drain.bv stateless
check_shape interval_step benchmarks/interval_step.bv stateless
check_shape matrix_pipeline benchmarks/matrix_pipeline.bv stateless
check_shape sweep_dense benchmarks/sweep_dense.bv stateless
check_shape mixed_effect_shape tests/tier1/mixed_effect_shape.bv escaped

# E1 diagnostic path: the stack plain-context ABI must remain semantically
# correct (it is default-off, but the experiment code is still compiled).
plain_float="$TMP/plain_float"
mkdir -p "$plain_float"
if ! BRIEV_DIAG_PLAIN_CONTEXT=1 BRIEV_DIAG_EFFECT=stateless \
    "$BRIEVC" build benchmarks/float_math.bv --out "$plain_float" \
    >"$plain_float/build.log" 2>&1; then
  echo "FAIL plain-context float_math: build failed" >&2
  FAIL=1
else
  BOUND=5000001 "$TMP/shape_float_math/float_math" >"$TMP/auto_float.out"
  BOUND=5000001 "$plain_float/float_math" >"$TMP/plain_float.out"
  if ! cmp -s "$TMP/auto_float.out" "$TMP/plain_float.out"; then
    echo "FAIL plain-context float_math: output differs from default" >&2
    FAIL=1
  else
    echo "ok   plain-context float_math: output matches default"
  fi
fi

# Mixed program coverage: the program-level selector escapes because `direct`
# is present, but the same program forced stateless must produce the same
# multiset of output. This catches a divergence between the two effect-memory
# shapes on a program that contains both loop kinds.
mixed_auto="$TMP/mixed_auto"
mixed_plain="$TMP/mixed_plain"
mkdir -p "$mixed_auto" "$mixed_plain"
if ! BRIEV_DIAG_EFFECT=auto "$BRIEVC" build tests/tier1/mixed_effect_shape.bv \
    --out "$mixed_auto" >"$mixed_auto/build.log" 2>&1; then
  echo "FAIL mixed_effect_shape: auto build failed" >&2
  FAIL=1
elif ! BRIEV_DIAG_EFFECT=stateless "$BRIEVC" build tests/tier1/mixed_effect_shape.bv \
    --out "$mixed_plain" >"$mixed_plain/build.log" 2>&1; then
  echo "FAIL mixed_effect_shape: forced-stateless build failed" >&2
  FAIL=1
else
  BOUND=1000001 "$mixed_auto/mixed_effect_shape" >"$TMP/mixed_auto.out" 2>"$TMP/mixed_auto.err"
  BOUND=1000001 "$mixed_plain/mixed_effect_shape" >"$TMP/mixed_plain.out" 2>"$TMP/mixed_plain.err"
  sort "$TMP/mixed_auto.out" >"$TMP/mixed_auto.sorted"
  sort "$TMP/mixed_plain.out" >"$TMP/mixed_plain.sorted"
  if ! cmp -s "$TMP/mixed_auto.sorted" "$TMP/mixed_plain.sorted"; then
    echo "FAIL mixed_effect_shape: escaped and stateless outputs differ" >&2
    diff -u "$TMP/mixed_auto.sorted" "$TMP/mixed_plain.sorted" | head -40 >&2 || true
    FAIL=1
  else
    echo "ok   mixed_effect_shape: escaped/stateless output multisets match"
  fi
fi

if [[ "$TIMING" == "1" ]]; then
  check_timing() {
    local name="$1" max="$2"
    local log="$TMP/time_$name.log"
    if ! bash "$ROOT/benchmarks/build_and_bench.sh" "$name" >"$log" 2>&1; then
      echo "FAIL $name: harness failed (see $log)" >&2
      FAIL=1
      return
    fi
    local ratio
    ratio="$(awk '/^[[:space:]]*Ratio:/ { gsub(/x/, "", $2); print $2; exit }' "$log")"
    if [[ -z "$ratio" ]]; then
      echo "FAIL $name: no Ratio row in harness output" >&2
      FAIL=1
      return
    fi
    if ! awk -v got="$ratio" -v max="$max" 'BEGIN { exit !(got <= max) }'; then
      echo "FAIL $name: ratio ${ratio}x exceeds catastrophic-regression ceiling ${max}x" >&2
      FAIL=1
      return
    fi
    echo "ok   $name: ${ratio}x (ceiling ${max}x)"
  }

  check_timing float_math 1.25
  check_timing telemetry_stream 1.25
  check_timing pid_control 1.20
  check_timing matrix_pipeline 1.15
  check_timing queue_drain 0.80
  check_timing interval_step 0.90
fi

if [[ "$FAIL" != "0" ]]; then
  echo "effect-shape guard: FAILED" >&2
  exit 1
fi
echo "effect-shape guard: PASS"
