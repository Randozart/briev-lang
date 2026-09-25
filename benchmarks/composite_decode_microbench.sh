#!/usr/bin/env bash
# composite_decode_microbench.sh — Front D A/B timing (plan
# 2026-09-24-followup-stages Stage 1): per-step decode timing for the
# ONE-launch composite attention kernel (qk→softmax→pv fused), the shape
# whose deferred-region matcher Front D asks to retire.
#
# Per step: rotate q (host fill + push_ranges), reset the work counter,
# one briev_accel_launch_resident(0, state, H). K/V are seeded once — the
# kernel reads all NKV rows regardless of content, so row churn costs
# nothing but noise. Reports p10/p50/p90 of push and launch.
#
# Usage: bash benchmarks/composite_decode_microbench.sh [D] [H] [HKV] [NKV] [REPS]
#        env TEMPLATE (default attention_decode_composite.abv)
#        env BRIEVC_FLAGS (e.g. --config-dir <dir> for the knob A/B)
#        env OUTDIR (keep the build; env NO_RUN=1 builds without running)
set -euo pipefail
cd "$(dirname "$0")/.."

D="${1:-128}"; H="${2:-32}"; HKV="${3:-8}"; NKV="${4:-4096}"; REPS="${5:-200}"
[ $((H % HKV)) -eq 0 ] || { echo "H=$H not divisible by HKV=$HKV" >&2; exit 1; }
TEMPLATE="${TEMPLATE:-examples/gpu/attention_decode_composite.abv}"
BRIEVC=./target/release/brievc
OUT="${OUTDIR:-$(mktemp -d /tmp/opencode/cmpbench.XXXX)}"
echo "artifacts: $OUT"

python3 benchmarks/attn_instantiate.py \
    --d "$D" --h "$H" --hkv "$HKV" --nkv "$NKV" \
    --template "$TEMPLATE" \
    --out examples/gpu/cmpbench_tmp.abv
"$BRIEVC" build examples/gpu/cmpbench_tmp.abv $BRIEVC_FLAGS --out "$OUT" >/dev/null
mv "$OUT/cmpbench_tmp_runner.c" "$OUT/cmp_runner.c"
rm -f examples/gpu/cmpbench_tmp.abv

python3 - "$OUT/cmp_runner.c" "$OUT/cmp_briev.c" "$D" "$H" "$HKV" "$NKV" "$REPS" <<'PYEOF'
import re, sys

runner_path, out_path = sys.argv[1], sys.argv[2]
D, H, HKV, NKV, REPS = (int(x) for x in sys.argv[3:8])
G = H // HKV
src = open(runner_path).read()

fields = {}
for m in re.finditer(r'\{ "(\w+)", (\d+), (\d+), (\d+), (\d+), ([01]), (\d+) \}', src):
    fields[m.group(1)] = {"host": int(m.group(3)), "proj": int(m.group(7))}
kernels = re.findall(r'\{ "(\w+)", k\d+,', src)
assert len(kernels) == 1, f"composite must be one kernel, found {kernels}"
QOFF, QPROJ = fields["q"]["host"], fields["q"]["proj"]
ROFF, RPROJ = fields["r"]["host"], fields["r"]["proj"]
QLEN = H * D
N_KERNELS = re.search(r'static const uint32_t N_KERNELS = (\d+);', src).group(1)
STATE_BYTES = re.search(r'state\[([0-9]+)\]', src)
state_decl = "uint8_t* state" if re.search(r'uint8_t\s*\*?\s*state\s*=', src) else "long long* state"

main = '''
#include <time.h>
static double cmp_now(void) {{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e6 + (double)ts.tv_nsec / 1e3;
}}
static int cmp_cmp(const void* a, const void* b) {{
    double x = *(const double*)a, y = *(const double*)b;
    return (x > y) - (x < y);
}}

int main(void) {{
    if (!briev_accel_init(descs, {NK})) {{ fprintf(stderr, "briev: no device\\n"); return 1; }}
    float* m4_qrow = malloc((size_t){QLEN} * 4);
    /* seed k/v deterministically once — the kernel reads all NKV rows;
       content churn costs nothing but scheduling noise */
    {{
        unsigned rng = 4242u;
        float* q_ = (float*)(state + {QOFF});
        for (int i = 0; i < {QLEN}; i++) {{ rng = rng*1103515245u+12345u; q_[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }}
    }}
    /* warmup: one real step triggers the program-level seed (full push) */
    *(long long*)(state + {ROFF}) = 0;
    if (!briev_accel_launch_resident(0, state, {H})) {{ fprintf(stderr, "warmup failed\\n"); return 1; }}

    double* push_t = malloc(sizeof(double) * {REPS});
    double* launch_t = malloc(sizeof(double) * {REPS});
    double* total_t = malloc(sizeof(double) * {REPS});
    for (int step = 0; step < {REPS}; step++) {{
        unsigned rng = 999u + (unsigned)step;
        for (int i = 0; i < {QLEN}; i++) {{
            rng = rng * 1103515245u + 12345u;
            m4_qrow[i] = (float)((rng >> 16) % 997) / 997.0f - 0.5f;
        }}
        memcpy(state + {QOFF}, m4_qrow, (size_t){QLEN} * 4);
        *(long long*)(state + {ROFF}) = 0;
        size_t ranges[6] = {{
            (size_t){QPROJ}, (size_t){QOFF}, (size_t){QLEN} * 4,
            (size_t){RPROJ}, (size_t){ROFF}, 8,
        }};
        double t0 = cmp_now();
        if (!briev_accel_push_ranges(state, ranges, 2)) {{ fprintf(stderr, "push failed\\n"); return 1; }}
        double t1 = cmp_now();
        if (!briev_accel_launch_resident(0, state, {H})) {{ fprintf(stderr, "launch failed\\n"); return 1; }}
        double t2 = cmp_now();
        push_t[step] = t1 - t0;
        launch_t[step] = t2 - t1;
        total_t[step] = t2 - t0;
    }}
    qsort(push_t, {REPS}, sizeof(double), cmp_cmp);
    qsort(launch_t, {REPS}, sizeof(double), cmp_cmp);
    qsort(total_t, {REPS}, sizeof(double), cmp_cmp);
    printf("composite decode D={D} H={H} HKV={HKV} NKV={NKV}:\\n");
    printf("  push   p10=%.1f p50=%.1f p90=%.1f us\\n",
           push_t[{REPS}/10], push_t[{REPS}/2], push_t[{REPS}*9/10]);
    printf("  launch p10=%.1f p50=%.1f p90=%.1f us\\n",
           launch_t[{REPS}/10], launch_t[{REPS}/2], launch_t[{REPS}*9/10]);
    printf("  step   p10=%.1f p50=%.1f p90=%.1f us\\n",
           total_t[{REPS}/10], total_t[{REPS}/2], total_t[{REPS}*9/10]);
    free(push_t); free(launch_t); free(total_t); free(m4_qrow);
    briev_accel_shutdown();
    return 0;
}}
'''
main = main.format(NK=N_KERNELS, QLEN=QLEN, QOFF=QOFF, QPROJ=QPROJ,
                   ROFF=ROFF, RPROJ=RPROJ, H=H, D=D, HKV=HKV, NKV=NKV, REPS=REPS)
prefix = src.split('int main(void) {')[0]
open(out_path, 'w').write(prefix + main)
print("composite shim injected", file=sys.stderr)
PYEOF

cc -O2 -I"$OUT" -L"$OUT" -o "$OUT/cmp_briev" "$OUT/cmp_briev.c" -lbriev_accel_rt -lbriev_gpu_rt -lvulkan -lOpenCL -lcuda -lpthread -lm
[ "${NO_RUN:-0}" = "1" ] && { echo "built: $OUT/cmp_briev"; exit 0; }
echo "== BRIEV (CUDA lane) =="
BRIEV_ACCEL_DEVICE=cuda "$OUT/cmp_briev"
