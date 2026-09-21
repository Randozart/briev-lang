#!/usr/bin/env bash
# softmax_gate.sh — comptime-fold composite gate (plan
# 2026-09-21-comptime-fold-expansion).
#
# Builds a softmax_fused! fixture, injects a double-precision reference
# validation into the generated runner, and runs BOTH device lanes.
# Exercises the shape-adaptive composite: the caller's `D` const folds
# `match dlen <= 32` at expansion — a small-span fixture takes the ONLINE
# arm, a large-span fixture the DEFERRED arm. Both must match the same
# reference.
#
# Usage: softmax_gate.sh <fixture.abv> <H> <NKV> <D>
set -uo pipefail

FIXTURE=$1
H=$2
NKV=$3
D=$4

OUT=$(mktemp -d /tmp/opencode/sfgate.XXXXXX)
NAME=$(basename "${FIXTURE%.abv}")

./target/release/brievc build "$FIXTURE" --out "$OUT" >/dev/null || {
    echo "build failed: $FIXTURE"; exit 1;
}
RUNNER="$OUT/${NAME}_runner.c"
[ -f "$RUNNER" ] || { echo "no runner generated"; exit 1; }

python3 - "$RUNNER" "$H" "$NKV" "$D" "$NAME" <<'PYEOF'
import re, sys

runner_path = sys.argv[1]
H, NKV, D = map(int, sys.argv[2:5])
NAME = sys.argv[5]
src = open(runner_path).read()
if '#include <math.h>' not in src:
    src = src.replace('#include', '#include <math.h>\n#include', 1)

def field(name):
    m = re.search(r'\{ "%s", 1, (\d+), (\d+),' % name, src)
    return int(m.group(1)), int(m.group(2))  # host_offset, elem_bytes

Q, QEB = field('q'); K, _ = field('k'); V, _ = field('v'); A, _ = field('a_out')
QLEN, KVLEN = H * D, H * NKV * D

seed = f'''
  {{ unsigned rng = 777;
    float* q = (float*)(state + {Q});
    float* k = (float*)(state + {K});
    float* v = (float*)(state + {V});
    for (int i = 0; i < {QLEN}; i++) {{ rng = rng*1103515245u + 12345u; q[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }}
    for (int i = 0; i < {KVLEN}; i++) {{ rng = rng*1103515245u + 12345u; k[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }}
    for (int i = 0; i < {KVLEN}; i++) {{ rng = rng*1103515245u + 12345u; v[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }}
  }}
'''

tail = f'''
  {{
    float* q = (float*)(state + {Q});
    float* k = (float*)(state + {K});
    float* v = (float*)(state + {V});
    float* A = (float*)(state + {A});
    double max_rel = 0.0; int bad = 0;
    for (int h = 0; h < {H}; h++) {{
        double mx = -1e300;
        double sc[{NKV}];
        for (int j = 0; j < {NKV}; j++) {{
            double a = 0.0;
            for (int d = 0; d < {D}; d++) a += (double)q[h*{D}+d] * (double)k[h*{NKV}*{D} + j*{D} + d];
            sc[j] = a; if (a > mx) mx = a;
        }}
        double sum = 0.0;
        for (int j = 0; j < {NKV}; j++) {{ sc[j] = exp(sc[j] - mx); sum += sc[j]; }}
        for (int d = 0; d < {D}; d++) {{
            double ref_ = 0.0;
            for (int j = 0; j < {NKV}; j++) ref_ += (sc[j]/sum) * (double)v[h*{NKV}*{D} + j*{D} + d];
            double got = (double)A[h*{D}+d];
            double err = fabs(got - ref_) / (fabs(ref_) + 1e-3);
            if (err > max_rel) max_rel = err;
            if (err > 1e-3 && bad < 3) {{ printf("  bad h=%d d=%d got=%g ref=%g\\n", h, d, got, ref_); bad++; }}
        }}
    }}
    printf("GATE {NAME}: max_rel=%.2e -> %s\\n",
           max_rel, max_rel < 1e-3 ? "PASS" : "FAIL");
  }}
'''

src = src.replace('  briev_accel_shutdown();', tail + '  briev_accel_shutdown();')
src = src.replace('  long guard = 0;', seed + '  long guard = 0;')
open(runner_path, 'w').write(src)
print("gate harness injected", file=sys.stderr)
PYEOF

cc -O2 -I"$OUT" -o "$OUT/gate" "$RUNNER" -lvulkan -lOpenCL -lcuda -lpthread -lm || {
    echo "gate compile failed"; exit 1;
}

echo "== CUDA lane =="
BRIEV_ACCEL_DEVICE=cuda "$OUT/gate" 2>/dev/null | grep GATE || true
echo "== VULKAN lane =="
BRIEV_ACCEL_DEVICE=vulkan "$OUT/gate" 2>/dev/null | grep GATE || true
echo "artifacts: $OUT"
