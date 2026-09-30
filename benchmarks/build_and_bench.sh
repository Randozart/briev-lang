#!/usr/bin/env bash
# Briev Optimization Benchmarks — Builds, times, and tags all benchmarks
#
# Every benchmark is tagged as either --runtime (FFI in hot loop, timing valid)
# or --optimizer (pure computation, LLVM may eliminate loop, timing meaningless).
# The harness detects precomputed binaries by size comparison and reports
# correctness for all.
#
# Usage:
#   bash benchmarks/build_and_bench.sh                          # all benchmarks
#   bash benchmarks/build_and_bench.sh --runtime                # runtime only
#   bash benchmarks/build_and_bench.sh --optimizer              # optimizer only
#   bash benchmarks/build_and_bench.sh --correctness            # output verification only
#   bash benchmarks/build_and_bench.sh iir_filter               # single benchmark
#   bash benchmarks/build_and_bench.sh --fuzz <N>               # with fuzzing

set -euo pipefail
cd "$(dirname "$0")/.."

MODE="all"
SELECTED_BENCH=""
FUZZ_N=""
CORRECTNESS_ONLY=false
DERIVE_MODE=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --fuzz) FUZZ_N="$2"; shift 2 ;;
        --runtime)   MODE="runtime"; shift ;;
        --optimizer) MODE="optimizer"; shift ;;
        --correctness) CORRECTNESS_ONLY=true; shift ;;
        --derive)    DERIVE_MODE=true; MODE="derive"; shift ;;
        all)   MODE="all"; shift ;;
        *)     SELECTED_BENCH="$1"; MODE="single"; shift ;;
    esac
done

# ── RESULT COLLECTION ───────────────────────────────────────────
# Collected as "name|briev|c|ratio|winner|correctness"
declare -a RESULTS=()
record_result() {
    RESULTS+=("$1|$2|$3|$4|$5|$6")
}

print_summary_table() {
    echo ""
    echo "╔═══════════════════════════╦════════════╦════════════╦══════════╦════════╦═══════════╗"
    echo "║ Benchmark                 ║ Briev      ║ C          ║ Ratio    ║ Winner ║ Correct   ║"
    echo "╠═══════════════════════════╬════════════╬════════════╬══════════╬════════╬═══════════╣"
    for entry in "${RESULTS[@]}"; do
        IFS='|' read -r name briev c ratio winner correct <<< "$entry"
        printf "║ %-25s ║ %-10s ║ %-10s ║ %-8s ║ %-6s ║ %-9s ║\n" \
            "$name" "$briev" "$c" "$ratio" "$winner" "$correct"
    done
    echo "╚═══════════════════════════╩════════════╩════════════╩══════════╩════════╩═══════════╝"
}

# ── TAGS ──────────────────────────────────────────────────────────────
# Every benchmark must be tagged as runtime or optimizer.
#   runtime:   FFI call in hot loop body → timing is meaningful
#   optimizer: All const inputs, no FFI in hot loop → LLVM may eliminate

declare -A TAG
# 2026-08-06 (accel plan): per-benchmark extra env + default timing bound.
# nbody_newton_accel needs BODYCOUNT (get_env_int! default 0 → empty sim) and a
# modest bound (each step is O(BODYCOUNT); 50M steps × 2048 bodies is ~1e11).
declare -A BENCH_ENV
BENCH_ENV[nbody_newton_accel]="BODYCOUNT=2048"
declare -A BENCH_BOUND
BENCH_BOUND[nbody_newton_accel]=50000
TAG[iir_filter]=optimizer
TAG[precompute_sum]=optimizer
TAG[const_heavy]=optimizer
TAG[async_counters_idio]=optimizer
TAG[ring_buffer]=runtime
TAG[async_counters_sym]=runtime
TAG[async_counters_runtime]=runtime
TAG[float_math]=runtime
TAG[float_math_nonzero]=runtime
TAG[sparse_dispatch]=runtime
TAG[print_loop]=runtime
TAG[nbody_newton]=runtime
TAG[nbody_newton_accel]=runtime
TAG[nbody_sqrt]=runtime
TAG[nbody_sqrt_idio]=runtime
TAG[fasta]=runtime
TAG[fannkuch_redux]=runtime
TAG[mandelbrot]=runtime
TAG[kalman_filter_runtime]=runtime
TAG[knucleotide]=runtime
TAG[cancel_math]=runtime
TAG[bridge_glue]=runtime
TAG[bridge_multi]=runtime
TAG[bit_clear]=runtime
TAG[queue_drain]=runtime
TAG[queue_drain_sym]=runtime
TAG[queue_drain_idio]=runtime
TAG[stack_push_pop]=runtime
TAG[interval_step]=runtime
TAG[gpu/saxpy]=runtime
TAG[iir_filter_runtime]=runtime
TAG[UTF8_ops]=optimizer
TAG[ring_buffer_runtime]=runtime
TAG[precompute_sum_runtime]=runtime
TAG[binary_trees]=runtime
TAG[meld-bridge]=runtime
TAG[meld-bridge-sym]=runtime
# 2026-07-31: Real-program periodic-guard benchmarks.
TAG[telemetry_stream]=runtime
TAG[pid_control]=runtime
TAG[matrix_pipeline]=runtime
TAG[accumulator_flush]=runtime
TAG[sweep_sparse]=runtime
TAG[sweep_mid]=runtime
TAG[sweep_dense]=runtime
TAG[sweep_arr]=runtime
TAG[series_converge]=runtime
TAG[global_lifetime]=runtime
TAG[deep_recursion]=runtime
TAG[arena_churn]=runtime
TAG[linked_list]=runtime
TAG[hash_ops]=runtime
TAG[hash_ops_idio]=runtime
TAG[enemy_swarm]=runtime

BENCHMARKS=(
    "iir_filter"
    "precompute_sum"
    "const_heavy"
    "async_counters_idio"
    "UTF8_ops"
    "ring_buffer"
    # "async_counters_sym"   # excluded from timing (40 min at 50M)
    "float_math"
    "float_math_nonzero"
    "sparse_dispatch"
    "print_loop"
    "nbody_newton"
    "nbody_newton_accel"   # accel entry-loop over bodies (Design A); needs BODYCOUNT env
    "nbody_sqrt"
    "nbody_sqrt_idio"
    "fasta"
    "fannkuch_redux"
    "mandelbrot"
    "kalman_filter_runtime"
    "knucleotide"
    "cancel_math"
    "bit_clear"
    "queue_drain"       # generic obj (RingBuffer<Int>) via std/collections.bv + countdown
    "queue_drain_sym"    # mirrors C step-for-step (enabled 2026-08-01: the missing import is fixed)
    "queue_drain_idio"   # Briev-native drain (enabled 2026-08-01: import { List } added; matches queue_drain_sym)
    "stack_push_pop"    # generic obj (Stack<Int, 256>) push/pop cycle via <- ops
    "interval_step"
    # 2026-07-31: Real-program periodic-guard benchmarks (countdown dispatch A/B).
    "telemetry_stream"
    "pid_control"
    "matrix_pipeline"
    "accumulator_flush"
    "sweep_sparse"
    "sweep_mid"
    "sweep_dense"
    "sweep_arr"     # Float[16] array-state sweep — array machinery is competitive (1.17x)
    "series_converge"   # watchdog liveliness — fires print_best(x) on convergence
    "global_lifetime"   # garbage-scheduled heap buffer (free after last consumer)
    "deep_recursion"    # runtime-depth recursion
    "arena_churn"       # bump-arena exhaustion + realloc-grow
    "linked_list"       # Malloc# heap nodes + pointer chasing
    "hash_ops"          # hash-indexed flat table ops
    "hash_ops_idio"     # idiomatic HashMap via std/collections.bv — restored
    #                     # 2026-08-18: the map is a real 2*N open-addressing
    #                     # table (never fills); the C reference mirrors that
    #                     # structure (was a 256-ring, a different workload).
    #                     # Verified MATCH at BOUND=50M, ~1.05x parity.
    "enemy_swarm"       # SoA reactive swarm (array-state)
    "bridge_glue"
    "bridge_multi"
    # "gpu/saxpy"        # no .bv file exists
    # "meld-bridge"      # no .bv file exists
    # "meld-bridge-sym"  # no .bv file exists
)

# ── DERIVE BENCHMARKS ─────────────────────────────────────────────────
# These benchmarks use derivation-only .bv files (:= { ... } without body).
# The pipeline: briev derive --stochastic → .opt.bv → compile → time.
# Results appear in a separate "MCMC Optimized" column.

DERIVE_BENCHMARKS=(
    "popcount_derive"
    "minmax_derive"
    "abs_derive"
)

# ── BUILD FUNCTIONS ───────────────────────────────────────────────────

build_bench() {
    local name="$1"

    echo ""
    echo "================================================"
    echo "  Building: $name  (tag: ${TAG[$name]})"
    echo "================================================"

    local bin="benchmarks/${name}"
    rm -f "$bin" "benchmarks/${name}.o" "benchmarks/${name}.ll" "benchmarks/${name}_c" "benchmarks/${name}_c.o"

    # 2026-07-27: Skip if no .bv file (e.g. bridge_glue uses Makefile).
    if [ ! -f "benchmarks/${name}.bv" ]; then
        echo "  No .bv source — custom build"
        return 0
    fi

    local budget=256
    local gpu_flag=""
    case "$name" in
        nbody_newton) budget=2048 ;;
        nbody_newton_accel) budget=2048 ;;
        nbody_sqrt)   budget=2048 ;;
        nbody_sqrt_idio) budget=2048 ;;
    esac

    # 2026-07-10: Set BOUND so getenv_int# at module init evaluates correctly.
    # Without this, benchmarks using getenv_int#("BOUND") get N=0 and all loops
    # are dead code (zero iterations → output "0" instead of correct checksum).
    # 2026-07-14: --llvm removed — compiler now produces binary by default.
    # 2026-07-26: Clear FFI cache + temp objects to avoid duplicate symbols.
    # The one-step `clang .ll lib/runtime/briev_rt.c` avoids cached .o conflicts.
    rm -f ~/.cache/briev-compiler/ffi/*.o /tmp/briev_rt*.o 2>/dev/null || true
    local bound="${BOUND:-50000000}"
    if [ -n "${BENCH_BOUND[$name]:-}" ]; then
        bound="${BENCH_BOUND[$name]}"
    fi
    if [ "${QUICK:-0}" = "1" ]; then
        case "$name" in
            nbody_newton|nbody_sqrt|nbody_sqrt_idio)
                bound=5000000
                ;;
        esac
    fi
    # 2026-08-06 (fix): `env` is required — a variable EXPANSION (e.g.
    # ${BENCH_ENV[$name]} -> "BODYCOUNT=2048") is treated by bash as a command
    # name, not an env assignment, so the direct `BOUND=.. $EXP cmd` form
    # fails with "BODYCOUNT=2048: command not found". `env` handles it.
    env BOUND="$bound" ${BENCH_ENV[$name]:-} ./target/release/brievc build "benchmarks/${name}.bv" \
        --out benchmarks --optimize-budget "$budget" $gpu_flag 2>&1

    if [ ! -f "$bin" ]; then
        # 2026-07-26: One-step link with briev_rt.c (no pre-compiled .o files).
        # This avoids duplicate symbol conflicts between FFI cache objects
        # and the harness's separate briev_rt.o.
        clang -O3 -flto -march=native -ffast-math -fdata-sections -ffunction-sections \
            -Wl,--gc-sections "benchmarks/${name}.ll" "lib/runtime/briev_rt.c" \
            -o "$bin" -lm 2>&1
    fi
    if [ -f "$bin" ]; then
        echo "  Briev binary ready."
    else
        echo "  (no binary — linking deferred)"
    fi
}

# ── DERIVE BUILD ──────────────────────────────────────────────────────

# Build a derivation benchmark: run briev derive --stochastic, then compile .opt.bv
build_derive_bench() {
    local name="$1"
    local bv_file="benchmarks/${name}.bv"
    local derive_opt="benchmarks/${name}.opt.bv"

    if [ ! -f "$bv_file" ]; then
        echo "  SKIP — no .bv source: $bv_file"
        return 1
    fi

    echo "  Deriving: $name"

    # Step 1: Run derivation + MCMC to produce .opt.bv
    BOUND=50000000 ./target/release/brievc derive --stochastic --iterations 10000 "$bv_file" 2>&1 | sed 's/^/    /'

    if [ ! -f "$derive_opt" ]; then
        echo "  SKIP — no .opt.bv produced (derive step may have failed)"
        return 1
    fi

    # Step 2: Build the .opt.bv into a binary
    echo "  Building MCMC-optimized binary..."
    rm -f ~/.cache/briev-compiler/ffi/*.o /tmp/briev_rt*.o 2>/dev/null || true
    BOUND=50000000 ./target/release/brievc build "$derive_opt" --out benchmarks 2>&1 | sed 's/^/    /'

    local bin="benchmarks/${name}_mcmc"
    if [ -f "benchmarks/${name}.ll" ]; then
        clang -O3 -flto -march=native -ffast-math -fdata-sections -ffunction-sections \
            -Wl,--gc-sections "benchmarks/${name}.ll" "lib/runtime/briev_rt.c" \
            -o "$bin" -lm 2>&1 | sed 's/^/    /'
    fi

    if [ -f "$bin" ]; then
        echo "  MCMC binary ready."
        return 0
    else
        echo "  SKIP — no MCMC binary produced"
        return 1
    fi
}

build_c() {
    local name=$1
    local src
    if [ -f "benchmarks/${name}_c.c" ]; then
        src="benchmarks/${name}_c.c"
    else
        # 2026-07-27: No direct C source — check for cross-reference.
        local ref_name="${BRIEV_CROSS_REF[$name]:-}"
        if [ -n "$ref_name" ]; then
            echo "  No C source — uses ${ref_name}_c for cross-reference timing"
        fi
        return 1
    fi

    # Build C reference with -O3 -ffast-math
    echo "  Building C reference..."
    extra_flags=${extra_flags:-""}
    clang -O3 -march=native -ffast-math -o "benchmarks/${name}_c" "$src" ${extra_flags} 2>&1
    echo "  C binary ready."
}

# ── TIMING HARNESS ────────────────────────────────────────────────────

TIMER_BIN="/tmp/briev_bench_timer"
TIMER_SRC="/tmp/briev_bench_timer.c"
if [ ! -f "$TIMER_BIN" ]; then
    cat > "$TIMER_SRC" << 'CEOF'
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <sys/wait.h>
#include <sys/resource.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc < 2) { fprintf(stderr, "usage: %s <cmd...>\n", argv[0]); return 1; }
    // 2026-07-21: Measure child CPU time (not wall clock) to exclude
    // fork+exec startup noise. Uses wait4() to get per-child rusage.
    // Combined with warmup run to preheat cache + dynamic linker.
    int ws, status; struct rusage wu, mu;
    // Warmup — fork+exec once, discard timing
    pid_t wup = fork();
    if (wup == 0) { freopen("/dev/null", "w", stdout); execvp(argv[1], &argv[1]); _exit(127); }
    wait4(wup, &ws, 0, &wu);
    // Measurement — fork+exec again, capture child CPU time
    pid_t pid = fork();
    if (pid == 0) { freopen("/dev/null", "w", stdout); execvp(argv[1], &argv[1]); _exit(127); }
    wait4(pid, &status, 0, &mu);
    // Report measurement child's user CPU time (excludes warmup, fork, wait)
    double elapsed = mu.ru_utime.tv_sec + mu.ru_utime.tv_usec / 1e6;
    printf("%.6f\n", elapsed); fflush(stdout);
    return WIFEXITED(status) ? WEXITSTATUS(status) : 1;
}
CEOF
    gcc -O2 -o "$TIMER_BIN" "$TIMER_SRC" 2>/dev/null
fi

# ── SIZE-GATED PRECOMPUTE DETECTION ──────────────────────────────────

# Returns true (0) if the benchmark is precompute_ok.
# Checks: briev binary .text size < 25% of C binary .text size,
#         or briev binary is missing (linking failed).
is_precompute_ok() {
    local name="$1"
    local briev_bin="benchmarks/${name}"
    local c_bin="benchmarks/${name}_c"

    # Optimizer-tagged benchmarks are always precompute_ok
    if [ "${TAG[$name]}" = "optimizer" ]; then
        return 0
    fi

    # Missing briev binary → no timing possible
    if [ ! -f "$briev_bin" ]; then
        return 0
    fi

    # Size comparison
    local briev_text=0
    local c_text=0
    if command -v size &>/dev/null; then
        briev_text=$(size "$briev_bin" 2>/dev/null | tail -1 | awk '{print $1}')
        if [ -f "$c_bin" ]; then
            c_text=$(size "$c_bin" 2>/dev/null | tail -1 | awk '{print $1}')
        fi
        if [ "$briev_text" -eq 0 ]; then
            return 0
        fi
        if [ "$c_text" -gt 0 ] && [ "$briev_text" -lt $(( c_text / 4 )) ]; then
            return 0
        fi
    fi

    return 1
}

# ── Cross-benchmark correctness references ──────────────────────────
# Some benchmarks (e.g. queue_drain_idio) have no C reference of their own —
# they are compared against a different benchmark's C reference (e.g.
# queue_drain_sym_c). The BRIEV_CROSS_REF array maps (benchmark, c_ref).
declare -A BRIEV_CROSS_REF
BRIEV_CROSS_REF["queue_drain_idio"]="queue_drain_sym"

# ── CORRECTNESS CHECK ────────────────────────────────────────────────

LAST_CORRECTNESS=""

check_correctness() {
    local name="$1"
    local briev_bin="benchmarks/${name}"

    local c_bin="benchmarks/${name}_c"
    local ref_name="${BRIEV_CROSS_REF[$name]:-$name}"
    local ref_c_bin="benchmarks/${ref_name}_c"

    if [ ! -f "$briev_bin" ]; then
        echo "  correctness: SKIP (briev binary missing)"
        LAST_CORRECTNESS="SKIP"
        return
    fi

    if [ "$name" != "$ref_name" ] && [ -f "$ref_c_bin" ]; then
        # Cross-benchmark comparison: compare against another benchmark's C ref
        :
    elif [ ! -f "$c_bin" ]; then
        echo "  correctness: SKIP (binary missing)"
        LAST_CORRECTNESS="SKIP"
        return
    fi

    local briev_out c_out briev_err c_err briev_rc c_rc
    local tdir
    tdir=$(mktemp -d) || tdir=/tmp
    # 2026-08-09 (Bug 5): capture STDOUT and STDERR separately. A runtime
    # error (divide-by-zero, undefined global, ...) prints diagnostics to
    # stderr; merging it into the compared output let a crashing program
    # "match" the C reference (deep_recursion printed a divide-by-zero and
    # reported MATCH). A non-empty stderr / non-zero exit is now FAIL, never
    # MATCH.
    #
    # 2026-09-30: the device runtime emits performance TELEMETRY on stderr
    # with the `# ` line prefix (`# gpu_time: …`, per-launch Vulkan
    # timestamps) — that is data for the timing sessions, not an error.
    # Crash diagnostics never use the `# ` prefix (`briev: dispatch
    # failed`, driver `[briev_accel/…]` messages, panics), so filtering
    # `^# ` keeps Bug 5's guarantee while GPU-engaged benchmarks (e.g.
    # nbody_newton_accel, which started dispatching Vulkan kernels after
    # the image-aware driver select) no longer false-FAIL on telemetry.
    env ${BENCH_ENV[$name]:-} BOUND=5 timeout 10 "$briev_bin" >"$tdir/briev.out" 2>"$tdir/briev.err"
    briev_rc=$?
    briev_out=$(cat "$tdir/briev.out" 2>/dev/null)
    # `|| true`: grep exits 1 when stderr is telemetry-only — under
    # pipefail that must not abort the harness (empty err = PASS path).
    briev_err=$(grep -v '^# ' "$tdir/briev.err" 2>/dev/null | head -c 200 || true)
    if [ "$name" != "$ref_name" ] && [ -f "$ref_c_bin" ]; then
        env ${BENCH_ENV[$name]:-} BOUND=5 timeout 10 "$ref_c_bin" >"$tdir/c.out" 2>"$tdir/c.err"
    else
        env ${BENCH_ENV[$name]:-} BOUND=5 timeout 10 "$c_bin" >"$tdir/c.out" 2>"$tdir/c.err"
    fi
    c_rc=$?
    c_out=$(cat "$tdir/c.out" 2>/dev/null)
    c_err=$(cat "$tdir/c.err" 2>/dev/null)
    rm -rf "$tdir"

    # A runtime error on either side is a hard FAIL — never a silent MATCH.
    if [ "$briev_rc" != "0" ] || [ -n "$briev_err" ]; then
        echo "  correctness: FAIL (briev runtime error, rc=$briev_rc stderr: \"${briev_err:0:80}\")"
        LAST_CORRECTNESS="FAIL"
        return
    fi
    if [ "$c_rc" != "0" ] || [ -n "$c_err" ]; then
        echo "  correctness: FAIL (C reference error, rc=$c_rc stderr: \"${c_err:0:80}\")"
        LAST_CORRECTNESS="FAIL"
        return
    fi

    if [ "$briev_out" = "$c_out" ]; then
        echo "  correctness: MATCH (output: \"${briev_out:0:40}\")"
        LAST_CORRECTNESS="MATCH"
        return
    fi

    # 2026-07-03: Epsilon-based float comparison. C auto-vectorizes,
    # changing f32 association order. Strict string compare produces
    # false MISMATCH for values differing by ~1e-7.
    # Compare each line numerically if all lines are floats.
    local briev_lines c_lines
    mapfile -t briev_lines <<< "$briev_out"
    mapfile -t c_lines <<< "$c_out"
    local n_briev=${#briev_lines[@]}
    local n_c=${#c_lines[@]}
    if [ "$n_briev" -ne "$n_c" ]; then
        echo "  correctness: MISMATCH (line count $n_briev vs $n_c)"
        LAST_CORRECTNESS="MISMATCH"
        return
    fi
    local all_float=true
    local i
    local re='^-?[0-9]+\.[0-9]+$'
    for ((i=0; i<n_briev; i++)); do
        if ! [[ "${briev_lines[$i]}" =~ $re ]] || ! [[ "${c_lines[$i]}" =~ $re ]]; then
            all_float=false
            break
        fi
    done
    if [ "$all_float" = false ]; then
        echo "  correctness: MISMATCH"
        echo "    briev: \"${briev_out:0:60}\""
        echo "    c:     \"${c_out:0:60}\""
        LAST_CORRECTNESS="MISMATCH"
        return
    fi
    # All lines are floats — compare with epsilon
    local eps=0.00001
    for ((i=0; i<n_briev; i++)); do
        local diff
        diff=$(LC_ALL=C python3 -c "b=${briev_lines[$i]}; c=${c_lines[$i]}; print('{:.15e}'.format(abs(b - c)))" 2>/dev/null)
        in_range=$(LC_ALL=C python3 -c "d=${diff}; print('yes' if d < $eps else 'no')" 2>/dev/null)
        if [ -z "$diff" ] || [ "$in_range" != "yes" ]; then
            echo "  correctness: MISMATCH (float diff $diff > $eps)"
            echo "    briev: \"${briev_out:0:60}\""
            echo "    c:     \"${c_out:0:60}\""
            LAST_CORRECTNESS="MISMATCH"
            return
        fi
    done
    echo "  correctness: MATCH (output: \"${briev_out:0:40}\")"
    LAST_CORRECTNESS="MATCH"
}

# ── BENCHMARK RUNNER ─────────────────────────────────────────────────

bench_self_term() {
    local name="$1"
    local briev_bin="benchmarks/${name}"
    local c_bin="benchmarks/${name}_c"
    # 2026-07-27: Cross-reference C binary — for benchmarks like queue_drain_idio
    # that compare against a different benchmark's C reference (queue_drain_sym_c).
    local ref_name="${BRIEV_CROSS_REF[$name]:-$name}"
    local ref_c_bin="benchmarks/${ref_name}_c"

    echo ""
    echo "=== $name ==="

    # 2026-07-22: Bridge benchmark uses Python harness, not compiled binary
    if [ "$name" = "bridge_glue" ]; then
        local py_bench="benchmarks/bridge/bench_glue_cross.py"
        if [ -f "$py_bench" ] && [ -x "$(command -v python3)" ]; then
            # 2026-08-09 (Bug 5): honor the Python harness's verdict — a wrong
            # bridge result exits 1 (FAIL), never SKIP.
            if python3 "$py_bench"; then
                LAST_CORRECTNESS="MATCH"
            else
                LAST_CORRECTNESS="FAIL"
            fi
            record_result "$name" "done" "" "" "" "$LAST_CORRECTNESS"
        else
            echo "  SKIP — no Python harness"
            record_result "$name" "SKIP" "" "" "" "SKIP"
        fi
        return
    fi

    # 2026-07-24: Multi-language bridge benchmark
    if [ "$name" = "bridge_multi" ]; then
        echo "  Python ctypes + protocol, Node.js koffi + protocol, shell protocol"
        local multi_dir="benchmarks/multi_lang"
        if [ -d "$multi_dir" ] && [ -x "$(command -v python3)" ]; then
            python3 "$multi_dir/bench_multi_lang.py" 2>&1 | sed 's/^/    /'
        else
            echo "  SKIP — no Python"
        fi
        if [ -d "$multi_dir" ] && [ -x "$(command -v node)" ]; then
            # 2026-08-01: Guard against node bench failure (missing koffi) —
            # under `set -e` a failing node bench aborted the whole suite before
            # the summary table. A broken benchmark must not kill the run.
            node "$multi_dir/bench_node.mjs" 2>&1 | sed 's/^/    /' || echo "    SKIP — node bench failed (missing koffi?)"
        else
            echo "  SKIP — no Node.js"
        fi
        if [ -d "$multi_dir" ] && [ -x "$(command -v bash)" ] && [ -x "$multi_dir/bench_shell.sh" ]; then
            bash "$multi_dir/bench_shell.sh" 2>&1 | sed 's/^/    /'
        fi
        record_result "$name" "done" "" "" "" "PASS"
        return
    fi

    # Check for precomputed
    if is_precompute_ok "$name"; then
        local briev_text=0
        local c_text=1
    if command -v size &>/dev/null; then
        if [ -f "$briev_bin" ]; then
            briev_text=$(size "$briev_bin" 2>/dev/null | tail -1 | awk '{print $1}')
        fi
        if [ -f "$ref_c_bin" ]; then
            c_text=$(size "$ref_c_bin" 2>/dev/null | tail -1 | awk '{print $1}')
        fi
    fi
    if [ -f "$briev_bin" ]; then
        echo "  briev binary: ${briev_text:-0}B  (precompute_ok — skip runtime)"
    else
        echo "  briev binary: (no binary — linking issue)"
    fi
    echo "  c binary:     ${c_text:-0}B"
        check_correctness "$name"
        record_result "$name" "precomputed" "" "" "" "$LAST_CORRECTNESS"
        return
    fi

    if [ ! -f "$briev_bin" ]; then
        echo "  SKIP — no briev binary (linking issue)"
        record_result "$name" "SKIP" "" "" "" "SKIP"
        return
    fi
    # 2026-07-18: Cross-benchmark reference check
    if [ "$name" != "$ref_name" ] && [ -f "$ref_c_bin" ]; then
        # Cross-benchmark: C ref exists under a different name
        :
    elif [ ! -f "$c_bin" ]; then
        echo "  SKIP — no C binary"
        record_result "$name" "SKIP" "" "" "" "SKIP"
        return
    fi

    if [ "$CORRECTNESS_ONLY" = true ]; then
        check_correctness "$name"
        record_result "$name" "" "" "" "" "$LAST_CORRECTNESS"
        return
    fi

    local briev_sum=0; local briev_min=999999; local briev_max=0
    local c_sum=0

    local bound="${BOUND:-50000000}"
    if [ -n "${BENCH_BOUND[$name]:-}" ]; then
        bound="${BENCH_BOUND[$name]}"
    fi
    if [ "${QUICK:-0}" = "1" ]; then
        case "$name" in
            nbody_newton|nbody_sqrt|nbody_sqrt_idio)
                bound=5000000
                ;;
        esac
    fi

    for i in 1 2 3 4 5; do
        local bt=$(env ${BENCH_ENV[$name]:-} BOUND="$bound" "$TIMER_BIN" "$briev_bin")
        local ct=$(env ${BENCH_ENV[$name]:-} BOUND="$bound" "$TIMER_BIN" "$ref_c_bin")
        briev_sum=$(echo "$briev_sum + $bt" | bc)
        c_sum=$(echo "$c_sum + $ct" | bc)
        if (( $(echo "$bt < $briev_min" | bc -l) )); then briev_min=$bt; fi
        if (( $(echo "$bt > $briev_max" | bc -l) )); then briev_max=$bt; fi
    done

    local briev_avg=$(echo "scale=4; $briev_sum / 5" | bc)
    local c_avg=$(echo "scale=4; $c_sum / 5" | bc)

    local winner="—"
    local ratio="N/A"
    if [ "$c_avg" != "0.0000" ] && [ "$briev_avg" != "0.0000" ]; then
        ratio=$(echo "scale=2; $briev_avg / $c_avg" | bc)
        if (( $(echo "$ratio < 1.0" | bc -l) )); then
            winner="Briev"
        elif (( $(echo "$ratio > 1.0" | bc -l) )); then
            winner="C"
        else
            winner="~tie"
        fi
    elif [ "$briev_avg" = "0.0000" ] && [ "$c_avg" != "0.0000" ]; then
        ratio="Briev wins (O(1) fold)"
        winner="Briev"
    fi

    echo "  Briev: ${briev_avg}s  (min ${briev_min}s, max ${briev_max}s)"
    echo "  C:     ${c_avg}s"
    echo "  Ratio: ${ratio}x  →  ${winner} wins"

    check_correctness "$name"
    record_result "$name" "${briev_avg}s" "${c_avg}s" "${ratio}x" "$winner" "$LAST_CORRECTNESS"
}

# ── DERIVE BENCHMARK TIMING ───────────────────────────────────────────

bench_derive_self_term() {
    local name="$1"
    local mcmc_bin="benchmarks/${name}_mcmc"
    local c_bin="benchmarks/${name}_c"

    echo ""
    echo "=== ${name} (MCMC) ==="

    # Build C reference
    build_c "$name" || {
        echo "  SKIP — no C reference"
        record_result "$name" "SKIP" "SKIP" "" "" "SKIP"
        return
    }

    if [ ! -f "$mcmc_bin" ]; then
        echo "  SKIP — no MCMC binary"
        record_result "$name" "SKIP" "SKIP" "" "" "SKIP"
        return
    fi

    if [ "$CORRECTNESS_ONLY" = true ]; then
        # Check MCMC output against C reference
        local mcmc_out c_out
        mcmc_out=$(BOUND=5 timeout 10 "$mcmc_bin" 2>&1 || echo "__FAIL__")
        c_out=$(BOUND=5 timeout 10 "$c_bin" 2>&1 || echo "__FAIL__")
        if [ "$mcmc_out" = "$c_out" ]; then
            echo "  correctness: MATCH"
            record_result "$name" "" "" "" "" "MATCH"
        else
            echo "  correctness: MISMATCH (mcmc='${mcmc_out:0:40}' c='${c_out:0:40}')"
            record_result "$name" "" "" "" "" "MISMATCH"
        fi
        return
    fi

    local mcmc_sum=0; local mcmc_min=999999; local mcmc_max=0
    local c_sum=0

    for i in 1 2 3 4 5; do
        local mt=$(env BOUND=50000000 "$TIMER_BIN" "$mcmc_bin")
        local ct=$(env BOUND=50000000 "$TIMER_BIN" "$c_bin")
        mcmc_sum=$(echo "$mcmc_sum + $mt" | bc)
        c_sum=$(echo "$c_sum + $ct" | bc)
        if (( $(echo "$mt < $mcmc_min" | bc -l) )); then mcmc_min=$mt; fi
        if (( $(echo "$mt > $mcmc_max" | bc -l) )); then mcmc_max=$mt; fi
    done

    local mcmc_avg=$(echo "scale=4; $mcmc_sum / 5" | bc)
    local c_avg=$(echo "scale=4; $c_sum / 5" | bc)

    local winner="—"
    local ratio="N/A"
    if [ "$c_avg" != "0.0000" ] && [ "$mcmc_avg" != "0.0000" ]; then
        ratio=$(echo "scale=2; $mcmc_avg / $c_avg" | bc)
        if (( $(echo "$ratio < 1.0" | bc -l) )); then
            winner="MCMC"
        elif (( $(echo "$ratio > 1.0" | bc -l) )); then
            winner="C"
        else
            winner="~tie"
        fi
    elif [ "$mcmc_avg" = "0.0000" ] && [ "$c_avg" != "0.0000" ]; then
        ratio="MCMC wins (O(1) fold)"
        winner="MCMC"
    fi

    echo "  MCMC: ${mcmc_avg}s  (min ${mcmc_min}s, max ${mcmc_max}s)"
    echo "  C:    ${c_avg}s"
    echo "  Ratio: ${ratio}x  →  ${winner} wins"

    # Check correctness
    local mcmc_out c_out
    mcmc_out=$(BOUND=5 timeout 10 "$mcmc_bin" 2>&1 || echo "__FAIL__")
    c_out=$(BOUND=5 timeout 10 "$c_bin" 2>&1 || echo "__FAIL__")
    if [ "$mcmc_out" = "$c_out" ]; then
        echo "  correctness: MATCH"
        record_result "$name" "${mcmc_avg}s" "${c_avg}s" "${ratio}x" "$winner" "MATCH"
    else
        echo "  correctness: MISMATCH"
        echo "    mcmc: '${mcmc_out:0:60}'"
        echo "    c:    '${c_out:0:60}'"
        record_result "$name" "${mcmc_avg}s" "${c_avg}s" "${ratio}x" "$winner" "MISMATCH"
    fi
}

# ── FILTER ────────────────────────────────────────────────────────────

filter_name() {
    local name="$1"
    if [ "$MODE" = "single" ] && [ "$name" != "$SELECTED_BENCH" ]; then
        return 1
    fi
    if [ "$MODE" = "runtime" ] && [ "${TAG[$name]}" != "runtime" ]; then
        return 1
    fi
    if [ "$MODE" = "optimizer" ] && [ "${TAG[$name]}" != "optimizer" ]; then
        return 1
    fi
    return 0
}

# ── DERIVE BENCHMARK FILTER ──────────────────────────────────────────

filter_derive_name() {
    local name="$1"
    if [ "$MODE" = "single" ] && [ "$name" != "$SELECTED_BENCH" ]; then
        return 1
    fi
    return 0
}

# ── MAIN ──────────────────────────────────────────────────────────────

# 2026-07-01: Pre-build disabled — run `cargo build --release --bin brievc`
# manually before executing this script to avoid the long build hiding benchmark output.
#echo "=== Building Briev compiler (release) ==="
#cargo build --release --bin brievc 2>&1
#echo ""

# ── MAIN LOOP ─────────────────────────────────────────────────────────

if [ "$DERIVE_MODE" = false ]; then
for name in "${BENCHMARKS[@]}"; do
    filter_name "$name" || continue
    if [ "$CORRECTNESS_ONLY" = true ]; then
        continue  # skip build in correctness-only mode
    fi
    build_bench "$name"
    if [ "$name" = "bridge_glue" ]; then
        echo "  bridge_glue: building C + Briev .so files..."
        make -C benchmarks/bridge PROJECT_ROOT="$PWD" BRIDGE_DIR="$PWD/target/bridge_bench" BRIEVC="$PWD/target/release/brievc" all 2>&1 | sed 's/^/    /'
    elif [ "$name" = "bridge_multi" ]; then
        echo "  bridge_multi: building Briev .so + protocol shim..."
        make -C benchmarks/multi_lang PROJECT_ROOT="$PWD" BUILD_DIR="$PWD/target/multi_lang" BRIEVC="$PWD/target/release/brievc" all 2>&1 | sed 's/^/    /'
    else
        build_c "$name" || true
    fi
done
fi  # end of DERIVE_MODE=false gate

echo ""
echo "================================================"
echo "  RUNNING BENCHMARKS"
echo "================================================"

if [ "$DERIVE_MODE" = false ]; then
for name in "${BENCHMARKS[@]}"; do
    filter_name "$name" || continue
    bench_self_term "$name"
done
fi  # end of DERIVE_MODE=false gate

if [ -n "$FUZZ_N" ]; then
    echo ""
    echo "================================================"
    echo "  FUZZING (n=$FUZZ_N)"
    echo "================================================"
    for name in "${BENCHMARKS[@]}"; do
        filter_name "$name" || continue
        echo ""
        bash benchmarks/fuzz.sh "$name" --mode runtime --runs "$FUZZ_N"
    done
fi

# ── DERIVE MAIN LOOP ──────────────────────────────────────────────────

if [ "$DERIVE_MODE" = true ]; then
    echo ""
    echo "================================================"
    echo "  BUILDING DERIVE BENCHMARKS (MCMC)"
    echo "================================================"
    for name in "${DERIVE_BENCHMARKS[@]}"; do
        filter_derive_name "$name" || continue
        build_derive_bench "$name"
    done

    echo ""
    echo "================================================"
    echo "  BENCHMARKING DERIVE BENCHMARKS (MCMC)"
    echo "================================================"
    for name in "${DERIVE_BENCHMARKS[@]}"; do
        filter_derive_name "$name" || continue
        bench_derive_self_term "$name"
    done
fi

echo ""
echo "================================================"
echo "  SUMMARY"
echo "================================================"
echo "  5 iterations per benchmark, avg wall clock via CLOCK_MONOTONIC."
echo "  BOUND=${bound:-50000000}. Nanosecond-precision fork+exec timing harness."
echo "  Tags: runtime=FFI in hot loop, optimizer=precompute_ok."
echo "================================================"

if [ ${#RESULTS[@]} -gt 0 ]; then
    print_summary_table
fi
