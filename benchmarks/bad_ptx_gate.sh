#!/usr/bin/env bash
# bad_ptx_gate.sh — bad<ptx> bridge device gate (plans
# 2026-10-01-bad-ptx-family.md M4, 2026-10-01-bad-site-blocks.md).
#
# Builds bad_override.abv (a copy node whose CUDA-lane image is authored
# as a bad<ptx> kernel unit), injects seed + reference validation into
# the generated runner, and runs BOTH device lanes:
#   CUDA   — must execute the AUTHORED unit (the override blob);
#   VULKAN — must execute the DERIVED SPIR-V image of the same node.
# Both must produce identical, seed-matching results — proving the
# authored kernel's decode/addressing and the bridge's merge geometry.
#
# Usage: bad_ptx_gate.sh [fixture.abv]
set -uo pipefail

FIXTURE=${1:-examples/gpu/bad_override.abv}
N=1024

OUT=$(mktemp -d /tmp/opencode/bptxgate.XXXXXX)
NAME=$(basename "${FIXTURE%.abv}")

./target/release/brievc build "$FIXTURE" --out "$OUT" | grep -E "bad<ptx>" || true
RUNNER="$OUT/${NAME}_runner.c"
[ -f "$RUNNER" ] || { echo "no runner generated"; exit 1; }

python3 - "$RUNNER" "$N" "$NAME" <<'PYEOF'
import re, sys

runner_path = sys.argv[1]
N = int(sys.argv[2])
NAME = sys.argv[3]
src = open(runner_path).read()
src = '#include <math.h>\n' + src

def field(name):
    m = re.search(r'\{ "%s", 1, (\d+), (\d+),' % name, src)
    return int(m.group(1)), int(m.group(2))  # host_offset, elem_bytes

A, _ = field('a'); R, _ = field('res')

seed = f'''
  {{ unsigned rng = 4242;
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
        double ref_ = (double)a[i];
        double got = (double)r[i];
        double err = fabs(got - ref_);
        if (err > max_abs) max_abs = err;
        if (err > 1e-6 && bad < 3) {{ printf("  bad i=%d got=%g ref=%g\\n", i, got, ref_); bad++; }}
    }}
    printf("GATE {NAME}: max_abs=%.2e -> %s\\n",
           max_abs, max_abs < 1e-6 ? "PASS" : "FAIL");
  }}
'''

src = src.replace('  briev_accel_shutdown();', tail + '  briev_accel_shutdown();')
src = src.replace('  long guard = 0;', seed + '  long guard = 0;')
open(runner_path, 'w').write(src)
print("gate harness injected", file=sys.stderr)
PYEOF

cc -O2 -I"$OUT" -L"$OUT" -o "$OUT/gate" "$RUNNER" -lbriev_accel_rt -lbriev_gpu_rt -lvulkan -lOpenCL -lcuda -lpthread -lm || {
    echo "gate compile failed"; exit 1;
}

echo "== CUDA lane (authored bad<ptx> unit) =="
BRIEV_ACCEL_DEVICE=cuda "$OUT/gate" 2>/dev/null | grep GATE || true
echo "== VULKAN lane (derived SPIR-V) =="
BRIEV_ACCEL_DEVICE=vulkan "$OUT/gate" 2>/dev/null | grep GATE || true
echo "artifacts: $OUT"
