#!/usr/bin/env bash
# composite_decode_split_sweep.sh — 5a split-K measurement (plan
# 2026-09-30-5a-decode-attention-parallel.md, design option 1). Sweeps the
# declared `split<S>` deferred modifier over the decode composite: builds
# each variant, drives the CUDA two-launch contract (partial at n*S,
# combine at n — the generated runner's own sequence), reports launch
# p10/p50/p90 at high REPS (the clock-ramp protocol).
#
# Usage: composite_decode_split_sweep.sh [D] [H] [HKV] [NKV] [REPS]
set -euo pipefail
cd "$(dirname "$0")/.."
D="${1:-128}"; H="${2:-20}"; HKV="${3:-5}"; NKV="${4:-4096}"; REPS="${5:-1200}"
BRIEVC=./target/release/brievc
TEMPLATE=examples/gpu/attention_decode_composite.abv
echo "geometry: D=$D H=$H HKV=$HKV NKV=$NKV REPS=$REPS"
for S in none 2 4 8 16; do
  if [ "$S" = none ]; then MOD=""; else MOD="split<$S> "; fi
  sed "s/^async node fattn/${MOD}async node fattn/" "$TEMPLATE" > /tmp/opencode/cmp_sweep.abv
  OUT=$(mktemp -d /tmp/opencode/cmpsw.XXXX)
  python3 benchmarks/attn_instantiate.py --d "$D" --h "$H" --hkv "$HKV" --nkv "$NKV" \
      --template /tmp/opencode/cmp_sweep.abv --out examples/gpu/cmpbench_tmp.abv >/dev/null
  "$BRIEVC" build examples/gpu/cmpbench_tmp.abv ${BRIEVC_FLAGS:-} --out "$OUT" >/dev/null
  mv "$OUT/cmpbench_tmp_runner.c" "$OUT/cmp_runner.c"
  rm -f examples/gpu/cmpbench_tmp.abv
  python3 - "$OUT/cmp_runner.c" "$D" "$H" "$REPS" "$S" <<'PYEOF'
import re, sys
runner_path, D, H, REPS, S = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), sys.argv[5]
src = open(runner_path).read()
fields = {}
for m in re.finditer(r'\{ "(\w+)", \d+, (\d+), \d+, \d+, \d+, (\d+) \}', src):
    fields[m.group(1)] = int(m.group(2))
qoff, roff = fields["q"], fields["r"]
partial_idx = None
combine_idx = None
split_factor = None
for m in re.finditer(r'briev_accel_launch_resident\((\d+), state, n_\w+(?: \* (\d+))?\)', src):
    if m.group(2):
        partial_idx, split_factor = m.group(1), int(m.group(2))
    elif combine_idx is None:
        # the FIRST plain launch after the partial = the combine; later
        # plain launches are other lanes' full images (kidx 0).
        combine_idx = m.group(1)
nk = int(re.search(r'static const uint32_t N_KERNELS = (\d+);', src).group(1))
if partial_idx is not None and split_factor is not None:
    mode = 'split=' + str(split_factor)
    pair = [
        'if (!briev_accel_launch_resident(' + str(partial_idx) + ', state, ' + str(H * split_factor) + ')) { fprintf(stderr, "launch failed. "); return 1; }',
        'if (!briev_accel_launch_resident(' + str(combine_idx) + ', state, ' + str(H) + ')) { fprintf(stderr, "launch failed. "); return 1; }',
    ]
else:
    mode = 'no-split'
    pair = ['if (!briev_accel_launch_resident(0, state, ' + str(H) + ')) { fprintf(stderr, "launch failed. "); return 1; }']
drive_txt = '\n    '.join(pair)
main = '''
#include <time.h>
#include <stdlib.h>
static double now_us(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec * 1e6 + (double)ts.tv_nsec / 1e3;
}
static int dcmp(const void* a, const void* b) {
    double x = *(const double*)a, y = *(const double*)b;
    return (x > y) - (x < y);
}
int main(void) {
    if (!briev_accel_init(descs, NK)) { fprintf(stderr, "no device. "); return 1; }
    unsigned rng = 4242u;
    float* q_ = (float*)(state + QOFF);
    for (int i = 0; i < HD; i++) { rng = rng*1103515245u+12345u; q_[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }
    *(long long*)(state + ROFF) = 0;
    WARM
    double* lt = malloc(sizeof(double) * REPS);
    for (int step = 0; step < REPS; step++) {
        rng = rng*1103515245u+12345u;
        q_[step % HD] = (float)((rng>>16)%997)/997.0f - 0.5f;
        *(long long*)(state + ROFF) = 0;
        double t1 = now_us();
        STEP
        double t2 = now_us();
        lt[step] = t2 - t1;
    }
    qsort(lt, REPS, sizeof(double), dcmp);
    printf("@SWEEPTAG@ p10=%.1f p50=%.1f p90=%.1f us (n=%d)\\n", lt[REPS/10], lt[REPS/2], lt[REPS*9/10], REPS);
    return 0;
}
'''
main = (main.replace('@SWEEPTAG@', mode)
            .replace('NK', str(nk))
            .replace('QOFF', str(qoff))
            .replace('ROFF', str(roff))
            .replace('HD', str(H * D))
            .replace('REPS', str(REPS))
            .replace('WARM', drive_txt)
            .replace('STEP', drive_txt))
prefix = src.split('int main(')[0]
gen = runner_path.replace('cmp_runner.c', 'cmp_sweep_gen.c')
open(gen, 'w').write(prefix + main)
print(gen)
PYEOF
  cc -O2 -I"$OUT" -L"$OUT" -o "$OUT/sweep" "$OUT/cmp_sweep_gen.c" -lbriev_accel_rt -lbriev_gpu_rt -lvulkan -lOpenCL -lcuda -lpthread -lm
  "$OUT/sweep"
  rm -rf "$OUT"
done
