#!/usr/bin/env bash
# deferred_ab_gate.sh — variant-diff verification for the deferred
# softmax (gate-hardening plan P1, 2026-10-01). Builds the SAME fixture
# twice (ptx_deferred_online: 0 / : 1 via config-dir), runs both on the
# CUDA lane, compares the output buffers ELEMENT-WISE variant-vs-variant
# — two paths wrong in different ways diverge; a reference check on one
# arm only verifies nothing about the other.
#
# Usage: deferred_ab_gate.sh [fixture.abv] [H] [NKV] [D] [OUTFIELD]
set -uo pipefail
cd "$(dirname "$0")/.."
FIXTURE="${1:-examples/gpu/softmax_composite_s8.abv}"
H="${2:-8}"; NKV="${3:-256}"; D="${4:-128}"
OUTFIELD="${5:-a_out}"
NAME=$(basename "${FIXTURE%.abv}")
BRIEVC=./target/release/brievc

run_variant() {
    local ONLINE="$1" DIR="$2"
    mkdir -p "$DIR"
    cp config/ir-lowering.dbvl "$DIR/ir-lowering.dbvl"
    python3 - "$DIR/ir-lowering.dbvl" "$ONLINE" <<'PYEOF'
import sys
p, online = sys.argv[1], sys.argv[2]
t = open(p).read()
line = 'ptx_deferred_online: ' + online + ';'
if 'ptx_deferred_online' in t:
    import re
    t = re.sub(r'ptx_deferred_online: \d+;', line, t)
else:
    t = t.rstrip() + '\n' + line + '\n'
open(p, 'w').write(t)
PYEOF
    OUT=$(mktemp -d /tmp/opencode/abv.XXXX)
    "$BRIEVC" build "$FIXTURE" --config-dir "$DIR" --out "$OUT" >/dev/null || { echo "build failed (online=$ONLINE)"; return 1; }
    echo "$OUT"
}

# build both variants of the same fixture
OUTA=$(run_variant 0 /tmp/opencode/ab_cfg0) || exit 1
OUTB=$(run_variant 1 /tmp/opencode/ab_cfg1) || exit 1
RA=$(ls "$OUTA"/*_runner.c | head -1)
RB=$(ls "$OUTB"/*_runner.c | head -1)

# inject seed + dual-output dump into BOTH runners (same seed → same inputs)
python3 - "$RA" "$RB" "$H" "$NKV" "$D" "$OUTFIELD" <<'PYEOF'
import re, sys

def field(src, name):
    m = re.search(r'\{ "%s", 1, (\d+), (\d+),' % name, src)
    return int(m.group(1))

def field_count(src, name):
    # The declared element count from the runner's field table — the seed
    # must write EXACTLY the buffer (the s8 fixture's k/v are H·NKV·D, but
    # a GQA decode fixture's k/v are HKV·NKV·D: seeding H·NKV·D overruns
    # the static state and glibc aborts the run before the dumps).
    m = re.search(r'\{ "%s", 1, \d+, \d+, (\d+),' % name, src)
    return int(m.group(1))

H, NKV, D, OUTFIELD = int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5]), sys.argv[6]
main_bodies = []
for i, rp in enumerate(sys.argv[1:3]):
    src = open(rp).read()
    if '#include <math.h>' not in src:
        src = src.replace('#include', '#include <math.h>\n#include', 1)
    Q, K, V, O = field(src, 'q'), field(src, 'k'), field(src, 'v'), field(src, OUTFIELD)
    QN, KN, VN = (field_count(src, n) for n in ('q', 'k', 'v'))
    seed = f'''
  {{ unsigned rng = 20260101;
    float* q = (float*)(state + {Q});
    float* k = (float*)(state + {K});
    float* v = (float*)(state + {V});
    for (int i = 0; i < {QN}; i++) {{ rng = rng*1103515245u + 12345u; q[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }}
    for (int i = 0; i < {KN}; i++) {{ rng = rng*1103515245u + 12345u; k[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }}
    for (int i = 0; i < {VN}; i++) {{ rng = rng*1103515245u + 12345u; v[i] = (float)((rng>>16)%997)/997.0f - 0.5f; }}
  }}
'''
    tail = f'''
  {{
    FILE* f = fopen("{"/tmp/opencode/ab_out_" + str(i) + ".bin"}", "wb");
    float* o = (float*)(state + {O});
    fwrite(o, 4, {H*D}, f);
    fclose(f);
  }}
'''
    assert '  briev_accel_shutdown();' in src
    src = src.replace('  briev_accel_shutdown();', tail + '  briev_accel_shutdown();')
    src = src.replace('  long guard = 0;', seed + '  long guard = 0;')
    open(rp, 'w').write(src)
    print(f"variant {i} harness injected", file=sys.stderr)
PYEOF

for OUT in "$OUTA" "$OUTB"; do
    cc -O2 -I"$OUT" -L"$OUT" -o "$OUT/gate" $(ls "$OUT"/*_runner.c | head -1) \
        -lbriev_accel_rt -lbriev_gpu_rt -lvulkan -lOpenCL -lcuda -lpthread -lm || { echo "compile failed"; exit 1; }
done

BRIEV_ACCEL_DEVICE=cuda "$OUTA/gate" >/dev/null 2>&1 || echo "WARN: variant A (two-pass) nonzero exit"
BRIEV_ACCEL_DEVICE=cuda "$OUTB/gate" >/dev/null 2>&1 || echo "WARN: variant B (online) nonzero exit"

python3 - /tmp/opencode/ab_out_0.bin /tmp/opencode/ab_out_1.bin <<'PYEOF'
import struct, sys
a = open(sys.argv[1], 'rb').read()
b = open(sys.argv[2], 'rb').read()
n = min(len(a), len(b)) // 4
fa = struct.unpack(f'{len(a)//4}f', a)
fb = struct.unpack(f'{len(b)//4}f', b)
import math
max_diff = 0.0
first = None
for i in range(n):
    x, y = fa[i], fb[i]
    if math.isnan(x) or math.isnan(y):
        d = 1e300
    else:
        d = abs(x - y) / (abs(y) + 1e-3)
    if d > max_diff:
        max_diff = d
        if first is None and d > 1e-3:
            first = (i, x, y)
print(f"AB-GATE: elements={n} max_rel_diff={max_diff:.3e} -> {'PASS' if max_diff < 1e-3 else 'FAIL'}")
if first:
    i, x, y = first
    print(f"  first divergence at [{i}]: two-pass={x} online={y}")
PYEOF
echo "artifacts: $OUTA $OUTB"
