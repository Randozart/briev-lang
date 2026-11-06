#!/usr/bin/env bash
# Intel Panther Lake benchmark lane — Workstream B.
#
# Pattern (documented for the results file): the harness SSHes into the node.
# The node runs the gate/timing scripts; results are streamed back. No
# credentials live in this repo — BRIEV_PTL_SSH is a command prefix chosen by
# the operator, e.g.
#   BRIEV_PTL_SSH="ssh user@host"                        (key/config auth)
#   BRIEV_PTL_SSH="sshpass -p ... ssh -o ... user@host"  (explicit, ephemeral)
#
# Usage:
#   cargo build --release
#   BRIEV_PTL_SSH="ssh user@host" bash benchmarks/intel_ptl/run.sh
#
# Env:
#   BRIEV_PTL_SSH          ssh command prefix (default: ssh overlordx86-64)
#   BRIEV_PTL_WORK         remote work dir (default /tmp/briev-ptl)
#   BRIEV_PTL_BENCHES      comma list (default elem,gemm,sdpa)
#   BRIEV_PTL_RESULTS_JSON local results output path
#   BRIEVC                 local brievc (default target/release/brievc)

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SSH_CMD="${BRIEV_PTL_SSH:-ssh overlordx86-64}"
read -r -a SSH <<< "$SSH_CMD"
WORK="${BRIEV_PTL_WORK:-/tmp/briev-ptl}"
BRIEVC="${BRIEVC:-$ROOT/target/release/brievc}"
BENCHES="${BRIEV_PTL_BENCHES:-elem,gemm,sdpa}"
RESULTS_JSON="${BRIEV_PTL_RESULTS_JSON:-$ROOT/benchmarks/results/2026-11-06-intel-ptl-first-pass.json}"

command -v "${SSH[0]}" >/dev/null || { echo "missing command: ${SSH[0]}" >&2; exit 2; }
[ -x "$BRIEVC" ] || { echo "build brievc first: cargo build --release" >&2; exit 2; }

echo "== sync harness + stdlib + brievc -> $WORK =="
tar -C "$ROOT" -cf - lib config plugins benchmarks/intel_ptl \
    | "${SSH[@]}" "mkdir -p '$WORK' && tar -C '$WORK' -xf -"
tar -C "$(dirname "$BRIEVC")" -cf - "$(basename "$BRIEVC")" \
    | "${SSH[@]}" "mkdir -p '$WORK/bin' && tar -C '$WORK/bin' -xf -"

echo "== correctness gates + perf_counter samples =="
"${SSH[@]}" "bash -lc 'cd $WORK && python3 benchmarks/intel_ptl/harness.py --work $WORK/run --benches $BENCHES'"

echo "== build collector (Briev timing source) =="
"${SSH[@]}" "bash -lc 'cd $WORK && BRIEV_HOME=$WORK $WORK/bin/brievc build benchmarks/intel_ptl/collector.bv --stdlib-path lib --config-dir config --out $WORK/bin'"

run_collector() {
    local bench="$1" dev="$2" iters="$3" width="$4"
    local inputs=""
    case "$bench" in
        elem) inputs="OV_INPUT0=$WORK/run/elem_in0.bin" ;;
        gemm) inputs="OV_INPUT0=$WORK/run/gemm_in0.bin OV_INPUT1=$WORK/run/gemm_in1.bin" ;;
        sdpa) inputs="OV_INPUT0=$WORK/run/sdpa_in0.bin OV_INPUT1=$WORK/run/sdpa_in1.bin OV_INPUT2=$WORK/run/sdpa_in2.bin" ;;
        *) echo "unknown bench $bench" >&2; exit 2 ;;
    esac
    echo "-- collector $bench $dev (iters=$iters, width=${width}ns)"
    "${SSH[@]}" "bash -lc 'cd $WORK && OV_MODEL=$WORK/run/$bench.xml OV_DEVICE=$dev OV_ITERS=$iters OV_WIDTH_NS=$width $inputs $WORK/bin/collector'"
}

case ",$BENCHES," in *,elem,*) run_collector elem GPU 60 1000; run_collector elem CPU 8 1000 ;; esac
case ",$BENCHES," in *,gemm,*) run_collector gemm GPU 20 10000; run_collector gemm CPU 2 100000 ;; esac
case ",$BENCHES," in *,sdpa,*) run_collector sdpa GPU 12 10000; run_collector sdpa CPU 1 100000 ;; esac

mkdir -p "$(dirname "$RESULTS_JSON")"
"${SSH[@]}" "cat '$WORK/run/results.json'" > "$RESULTS_JSON"
echo "== results json: $RESULTS_JSON =="
