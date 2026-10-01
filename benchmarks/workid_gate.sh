#!/usr/bin/env bash
# workid_gate.sh — GetGlobalId# device gate (primitive-coverage gap #4).
# Injects a seeded `a` and checks res[i] == a[i] + i on BOTH lanes.
set -uo pipefail
FIXTURE=${1:-examples/gpu/workid.abv}
N=1024
OUT=$(mktemp -d /tmp/opencode/widgate.XXXXXX)
NAME=$(basename "${FIXTURE%.abv}")
./target/release/brievc build "$FIXTURE" --out "$OUT" >/dev/null || { echo "build failed"; exit 1; }
RUNNER="$OUT/${NAME}_runner.c"
[ -f "$RUNNER" ] || { echo "no runner generated"; exit 1; }
python3 - "$RUNNER" "$N" "$NAME" <<'PYEOF'
import re, sys
runner_path, N, NAME = sys.argv[1], int(sys.argv[2]), sys.argv[3]
src = '#include <math.h>\n' + open(runner_path).read()
def field(name):
    m = re.search(r'\{ "%s", 1, (\d+), (\d+),' % name, src)
    return int(m.group(1)), int(m.group(2))
A, _ = field('a'); R, _ = field('res')
seed = f'''
  {{ unsigned rng = 909;
    float* a = (float*)(state + {A});
    for (int i = 0; i < {N}; i++) {{ rng = rng*1103515245u + 12345u; a[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }}
  }}
'''
tail = f'''
  {{
    float* a = (float*)(state + {A});
    float* r = (float*)(state + {R});
    int bad = 0; double max_abs = 0.0;
    for (int i = 0; i < {N}; i++) {{
        // The kernel computes in f32 — the reference must round through
        // f32 identically (RN), else the gate measures f32 grid spacing
        // (~6.1e-05 at i≈1024), not kernel error.
        double ref_ = (double)((float)a[i] + (float)i);
        double got = (double)r[i];
        double err = fabs(got - ref_);
        if (err > max_abs) max_abs = err;
        if (err > 1e-6 && bad < 3) {{ printf("  bad i=%d got=%g ref=%g\\n", i, got, ref_); bad++; }}
    }}
    printf("GATE {NAME}: max_abs=%.2e -> %s\\n", max_abs, max_abs < 1e-6 ? "PASS" : "FAIL");
  }}
'''
src = src.replace('  briev_accel_shutdown();', tail + '  briev_accel_shutdown();')
src = src.replace('  long guard = 0;', seed + '  long guard = 0;')
open(runner_path, 'w').write(src)
print("gate harness injected", file=sys.stderr)
PYEOF
cc -O2 -I"$OUT" -L"$OUT" -o "$OUT/gate" "$RUNNER" -lbriev_accel_rt -lbriev_gpu_rt -lvulkan -lOpenCL -lcuda -lpthread -lm || { echo "gate compile failed"; exit 1; }
echo "== CUDA lane =="
BRIEV_ACCEL_DEVICE=cuda "$OUT/gate" 2>/dev/null | grep GATE || true
echo "== VULKAN lane =="
BRIEV_ACCEL_DEVICE=vulkan "$OUT/gate" 2>/dev/null | grep GATE || true
echo "artifacts: $OUT"
